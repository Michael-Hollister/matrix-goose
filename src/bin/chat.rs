use goose::prelude::*;
use rand::Rng;
use rand::seq::SliceRandom;

use tokio::{
    task::JoinHandle,
    time::{Duration, Instant},
    sync::RwLock,
};
use std::{
    collections::HashMap,
    sync::Arc,
};

use rand_distr::{Exp, LogNormal, Distribution};
use once_cell::sync::Lazy;
use weighted_rand::builder::*;
// use duration_string::DurationString;

// use matrix_sdk::Client;
use matrix_sdk::ruma::{
    TransactionId,
    OwnedRoomId,
};

use matrix_goose::{
    matrix::{
        GooseMatrixClient,
        GOOSE_USERS,

        config::SyncSettings,
    },
    task_sleep, CANCELED,
};


#[derive(Debug, Clone, serde::Deserialize)]
struct User {
    username: String,
    password: String,
}

#[derive(Debug)]
struct ClientData {
    room_id: Option<OwnedRoomId>,
    room_tokens: HashMap<OwnedRoomId, String>,
    sync_forever_handle: JoinHandle<()>,
}

// #[derive(Debug, Clone)]
enum TaskIndex {
    DoNothing,
    SendText,
    LookAtRoom,
    PaginateRoom,
    GoAFK,
    ChangeDisplayName,
    SendImage,
    SendReaction,
}

impl From<usize> for TaskIndex {
    fn from(value: usize) -> Self {
        match value {
            0 => Self::DoNothing,
            1 => Self::SendText,
            2 => Self::LookAtRoom,
            3 => Self::PaginateRoom,
            4 => Self::GoAFK,
            5 => Self::ChangeDisplayName,
            6 => Self::SendImage,
            7 => Self::SendReaction,
            _ => panic!("Invalid enum index"),
        }
    }
}

static mut USERS: Vec<User> = Vec::new();
static USERS_READER: &Vec<User> = unsafe { &USERS };

// Note that a single goose user reference may required shared ownership between two
// threads (sync_forever and logic thread) depending on the current state of the tokio
// runtime task scheduler.
static mut CLIENTS: Lazy<HashMap<usize, Arc<GooseMatrixClient>>> = Lazy::new(|| { HashMap::new() });
static ATTACK_START: Lazy<Instant> = Lazy::new(|| { Instant::now() });

const lorem_ipsum_text: &str = "Lorem ipsum dolor sit amet, consectetur adipiscing elit, sed do eiusmod tempor incididunt ut labore et dolore magna aliqua. Ut enim ad minim veniam, quis nostrud exercitation ullamco laboris nisi ut aliquip ex ea commodo consequat. Duis aute irure dolor in reprehenderit in voluptate velit esse cillum dolore eu fugiat nulla pariatur. Excepteur sint occaecat cupidatat non proident, sunt in culpa qui officia deserunt mollit anim id est laborum.";

async fn get_client(index: usize) -> Arc<GooseMatrixClient> {
    unsafe { return Arc::clone(&CLIENTS.get(&index).unwrap()) }
}

async fn setup(_user: &mut GooseUser) -> TransactionResult {
    println!("Setting up loadtest...");

    // Load users from csv
    unsafe {
        match csv::Reader::from_path("users.csv") {
            Ok(mut reader) => {
                for entry in reader.deserialize::<User>() {
                    match entry {
                        Ok(record) => {
                            // println!("{:?}", record);
                            USERS.push(record);
                        },
                        Err(err) => panic!("Error reading user from users.csv: {}", err),
                    }
                }
            },
            Err(err) => panic!("Error reading users.csv: {}", err),
        }

        // Resize CLIENTS to prevent multiple re-allocations
        CLIENTS.reserve(USERS.len());
    }

    Ok(())
}

async fn teardown(_user: &mut GooseUser) -> TransactionResult {
    println!("Tearing down loadtest...");

    Ok(())
}

async fn on_start(user: &mut GooseUser) -> TransactionResult {
    let user_index = user.weighted_users_index;
    unsafe {
        let host = user.base_url.to_owned();
        GOOSE_USERS.push(user);

        let static_client_ref = Arc::new(GooseMatrixClient::new(host, user_index).await.unwrap());
        CLIENTS.insert(user_index, static_client_ref);

        let client = Arc::clone(&CLIENTS[&user_index]);
        let csv_user = &USERS_READER[user_index];
        let username = csv_user.username.to_owned();
        let password = csv_user.password.to_owned();

        match client.login_username(&username, &password).send().await {
            Ok(_) => {
                println!("[{}] Logged in successfully", username);

                // Spawn sync_forever task
                let handle = tokio::spawn(async move {
                    // println!("Spawning sync_forever task");
                    let mut sync_settings = SyncSettings::default();

                    loop {
                        // We cannot use the SDK's `sync` forever method because we can
                        // get segmentation faults if we attempt to abort the task while
                        // having an open TCP connection
                        match client.sync_once(sync_settings.clone()).await {
                            Ok(response) => {
                                sync_settings = sync_settings.token(response.next_batch.clone());
                            },
                            // Sync timeout warnings already gets outputted to console and report
                            Err(_) => {},
                            // Err(err) => { println!("[{}] Sync error: {}", username, err) },
                        }

                        // Drop lock after checking canceled status
                        {
                            if *CANCELED.read().await {
                                break;
                            }
                        }
                    }
                });

                user.set_session_data(ClientData { room_id: None, room_tokens: HashMap::new(), sync_forever_handle: handle });

                Ok(())
            },
            Err(error) => {
                println!("[{}] Error logging in: {:?}", username, error);
                Ok(())
            }
        }
    }
}

async fn on_stop(user: &mut GooseUser) -> TransactionResult {
    // println!("Stopping goose user {}...", user.weighted_users_index);

    if let Some(client_data) = user.get_session_data::<ClientData>() {
        // Drop lock after updating canceled status
        {
            if !*CANCELED.read().await {
                *CANCELED.write().await = true;
            }
        }

        // We cannot directly abort sync_forever task since segmentation faults could
        // occur if task is aborted having an open TCP connection
        while !client_data.sync_forever_handle.is_finished() {
            // Wait until timeout expires or sync response is received
            task_sleep(1.0, false).await;
        }
    }

    Ok(())
}

// The Goose task scheduler is insufficient for our use-case:
//   1. Transaction ordering for a given scenario is deterministic and not random
//   2. Scenario scheduling can be set to 'random', but Goose allocates a set of users
//      only at start to a given scenario that ALWAYS runs the transactions within its
//      own scenario for the remainder of the program.
// Thus to replicate similar behavior to Locust, we have to create our own scheduler to
// achieve a weighted, non-deterministic selection of user actions
async fn task_scheduler(user: &mut GooseUser) -> TransactionResult {
    // let runtime: Duration = DurationString::from_string(user.config.run_time.to_owned()).unwrap().into();
    // Goose seems to internally convert the '--run-time' time string to seconds
    let runtime = Duration::from_secs(user.config.run_time.parse::<u64>().unwrap());

    // Drop lock after checking canceled status
    {
        // Goose seems to re-run the scheduler transaction upon the
        // GooseAttack phase decrease...
        if *CANCELED.read().await || ATTACK_START.elapsed() > runtime {
            return Ok(());
        }
    }

    // Scheduler setup
    let index_weights = [11, 3, 4, 1, 1, 1, 0, 0];
    let task_gen = WalkerTableBuilder::new(&index_weights).build();

    // Task scheduler loop
    loop {
        // Drop lock after checking canceled status
        {
            // Goose is unable to terminate users upon switching to the decrease phase
            if *CANCELED.read().await || ATTACK_START.elapsed() > runtime {
                break;
            }
        }

        let index = task_gen.next_rng(&mut rand::thread_rng());
        match TaskIndex::from(index) {
            TaskIndex::DoNothing => { let _ = do_nothing(user).await; },
            TaskIndex::SendText => { let _ = send_text(user).await; },
            TaskIndex::LookAtRoom => { let _ = look_at_room(user).await; },
            TaskIndex::PaginateRoom => { let _ = paginate_room(user).await; },
            TaskIndex::GoAFK => { let _ = go_afk(user).await; },
            TaskIndex::ChangeDisplayName => { let _ = change_displayname(user).await; },
            TaskIndex::SendImage => { let _ = send_image(user).await; },
            TaskIndex::SendReaction => { let _ = send_reaction(user).await; },
        }

        task_sleep(0.1, true).await;
    }

    Ok(())
}

async fn do_nothing(_user: &mut GooseUser) -> TransactionResult {
    let exp = Exp::new(0.1).unwrap();
    let delay = exp.sample(&mut rand::thread_rng());
    task_sleep(delay, true).await;

    Ok(())
}

async fn send_text(user: &mut GooseUser) -> TransactionResult {
    let user_index = user.weighted_users_index;
    let client = get_client(user_index).await;
    let username = client.user_id().unwrap().localpart();
    use ruma::api::client::typing::create_typing_event::v3::Request as TypingRequest;
    use ruma::api::client::typing::create_typing_event::v3::Typing as Typing;
    use ruma::api::client::message::send_message_event::v3::Request as MessageRequest;
    use ruma::events::room::message::RoomMessageEventContent as RoomMessage;

    // Send the typing notification like a real client would
    let user_id = client.user_id().unwrap().to_owned();
    let room_id;

    match user.get_session_data::<ClientData>().unwrap().room_id.to_owned() {
        Some(id) => {room_id = id.to_owned(); },
        None => return Ok(()),
    }

    let typing = Typing::Yes(Duration::from_secs(30));
    let request = TypingRequest::new(user_id, room_id.to_owned(), typing);
    if client.send(request, None).await.is_err() {
        println!("[{}] failed sending typing notification", username);
    }

    // Sleep while we pretend the user is banging on the keyboard
    let exp = Exp::new(1.0 / 5.0).unwrap();
    let delay = exp.sample(&mut rand::thread_rng());
    task_sleep(delay, true).await;

    let words: Vec<&str> = lorem_ipsum_text.split(" ").collect();
    let log_normal = LogNormal::new(1.0, 1.0).unwrap();
    let mut message_len = f64::round(log_normal.sample(&mut rand::thread_rng())) as usize;
    message_len = usize::max(usize::min(message_len, words.len()), 1);


    let content = RoomMessage::text_plain(words[0 .. message_len].join(" "));
    let request = MessageRequest::new(room_id.to_owned(), TransactionId::new(), &content).unwrap();
    if client.send(request, None).await.is_err() {
        println!("[{}] failed to send/chat in room [{}]", username, room_id);
    }

    Ok(())
}

async fn look_at_room(user: &mut GooseUser) -> TransactionResult {
    let user_index = user.weighted_users_index;
    let client = get_client(user_index).await;
    let username = client.user_id().unwrap().localpart();
    use ruma::api::client::message::send_message_event::v3::Request as MessageRequest;
    use ruma::events::room::message::RoomMessageEventContent as RoomMessage;


    // room_id = self.get_random_roomid()
    // if room_id is None:
    //     #logging.warning("User [%s] couldn't get a roomid for look_at_room" % self.username)
    //     return
    // #logging.info("User [%s] looking at room [%s]" % (self.username, room_id))

    // self.load_data_for_room(room_id)

    // if len(self.recent_messages.get(room_id, [])) < 1:
    //     return

    // event_id = self.recent_messages[room_id][-1].event_id
    // self.matrix_client.update_receipt_marker(room_id, event_id)


    Ok(())
}

// # FIXME Combine look_at_room() and paginate_room() into a TaskSet,
// #       so the user can paginate and scroll the room for a longer
// #       period of time.
// #       In this model, we should load the displaynames and avatars
// #       and message thumbnails every time we paginate, just like a
// #       real client would do as the user scrolls the timeline.
async fn paginate_room(user: &mut GooseUser) -> TransactionResult {
    let user_index = user.weighted_users_index;
    let client = get_client(user_index).await;
    let username = client.user_id().unwrap().localpart();

    let room_id;
    match client.joined_rooms().choose(&mut rand::thread_rng()) {
        Some(joined) => room_id = joined.room_id().to_owned(),
        None => return Ok(()),
    }

    let client_data = user.get_session_data_mut::<ClientData>().unwrap();
    client_data.room_id = Some(room_id.to_owned());

    // Note: consider swapping this with the client API call instead? no need to keep track of tokens
    use ruma::api::client::message::get_message_events::v3::Request as Request;
    use ruma::api::Direction as Direction;

    let mut request = Request::new(room_id.to_owned(), Direction::Backward);
    if let Some(token) = client_data.room_tokens.get(&room_id) {
        request.from = Some(token.to_owned());
    }

    match client.send(request, None).await {
        Ok(response) => {
            if let Some(token) = response.end {
                // println!("[{}] Setting room token - Old: {:?}, New: {}", username, client_data.room_tokens.get(&room_id.to_owned()), token);
                client_data.room_tokens.insert(room_id, token);
            }
        },
        Err(_) => println!("[{}] failed /messages failed for room [{}]", username, room_id),
    }

    Ok(())
}

async fn go_afk(user: &mut GooseUser) -> TransactionResult {
    let user_index = user.weighted_users_index;

    let csv_user = &USERS_READER[user_index];
    let username = &csv_user.username.to_owned();
    println!("[{}] going away from keyboard", username);

    // Generate large(ish) random away time.
    let exp = Exp::new(1.0 / 600.0).unwrap(); // Expected value = 10 minutes
    let delay = exp.sample(&mut rand::thread_rng());
    task_sleep(delay, true).await;

    Ok(())
}

async fn change_displayname(user: &mut GooseUser) -> TransactionResult {
    let user_index = user.weighted_users_index;
    let client = get_client(user_index).await;
    let username = client.user_id().unwrap().localpart();
    use ruma::api::client::profile::set_display_name::v3::Request as Request;

    let user_number = *username.split(".").collect::<Vec<&str>>().last().unwrap();
    let random_number = rand::thread_rng().gen_range(1 .. 1000);
    let new_name = format!("User {} (random={})", user_number, random_number);

    let request = Request::new(client.user_id().unwrap().to_owned(), Some(new_name.to_owned()));
    if client.send(request, None).await.is_err() {
        println!("[{}] failed to set displayname to {}", username, new_name);
    }

    Ok(())
}

async fn send_image(user: &mut GooseUser) -> TransactionResult {
    // # Choose an image to send/upload
    // # Upload the thumbnail -- FIXME We need to have all of the thumbnails created and stored *before* we start the test.  Performance will be awful if we're trying to dynamically resample the images on-the-fly here in the load generator.
    // # Upload the image data, get back an MXC URL
    // # Craft the event JSON structure
    // # Send the event
    Ok(())
}

async fn send_reaction(user: &mut GooseUser) -> TransactionResult {
    let user_index = user.weighted_users_index;
    let client = get_client(user_index).await;
    let username = client.user_id().unwrap().localpart();
    use ruma::api::client::message::send_message_event::v3::Request as MessageRequest;
    use ruma::events::room::message::RoomMessageEventContent as RoomMessage;

    // Pick a recent message from the selected room, and react to it
    // let user_id = client.user_id().unwrap().to_owned();
    // let room_id;
    // match client.joined_rooms().choose(&mut rand::thread_rng()) {
    //     Some(joined) => room_id = joined.room_id().to_owned(),
    //     None => return Ok(()),
    // }

    // let messages = client.get_joined_room(room_id).unwrap().messages(options)
    // let room = client.joined_rooms()
    //                               .choose(&mut rand::thread_rng())
    //                               .unwrap().to_owned();
    // let room_id = room.room_id().to_owned();
    // if room.messages(options)



    // if len(self.user.recent_messages.get(self.room_id, [])) < 1:
    // return

    // message = random.choice(self.user.recent_messages[self.room_id])
    // reaction = random.choice(["💩","👍","❤️", "👎", "🤯", "😱", "👏"])
    // content = {
    //     "m.relates_to": {
    //         "rel_type": "m.annotation",
    //         "event_id": message.event_id,
    //         "key": reaction,
    //     }
    // }

    // Prevent errors with reacting to the same message with the same reaction
    // if (message, reaction) in self.reacted_messages:
    //     return
    // else:
    //     self.reacted_messages.append((message, reaction))


    // let content = RoomMessage::text_plain(words[0 .. message_len].join(" "));
    // let content = RoomMessage::
    // let request = MessageRequest::new(room_id.to_owned(), TransactionId::new(), &content).unwrap();
    // if client.send(request, None).await.is_err() {
    //     println!("[{}] failed to send reaction in room [{}]", username, room_id);
    // }

    // response = self.user.matrix_client.room_send(self.room_id, "m.reaction", content)
    // if isinstance(response, RoomSendError):
        // logging.error("[%s] failed to send reaction in room [%s]: Code=%s, Message=%s",
                    //   self.user.matrix_client.user, response.room_id, response.status_code, response.message)

    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), GooseError> {
    println!("Starting matrix user chat loadtest...");

    // Run test
    GooseAttack::initialize()?
    .test_start(transaction!(setup))
    .register_scenario(scenario!("Default")
        .register_transaction(transaction!(on_start)
            .set_on_start()
            .set_name("On start"))
        .register_transaction(transaction!(task_scheduler)
            .set_name("Scheduler"))
        .register_transaction(transaction!(on_stop)
            .set_on_stop()
            .set_name("On stop"))
        .set_wait_time(Duration::from_millis(100), Duration::from_millis(200))?
    )
    .test_stop(transaction!(teardown))
    .set_default(GooseDefault::HatchRate, "32")?
    .execute()
    .await?;

    Ok(())
}

