use goose::prelude::*;
use rand::Rng;
use rand::seq::SliceRandom;


// use std::{future::Future, pin::Pin};

// use std::thread;
use tokio::{
    task::JoinHandle,
    time::{sleep, Duration},
    sync::RwLock,
};
use std::{
    collections::HashMap,
    sync::Arc,
};

use rand_distr::{Exp, LogNormal, Distribution};
use lazy_static::lazy_static;
use once_cell::sync::Lazy;

// use matrix_sdk::Client;
use matrix_sdk::ruma::{
    // api::client::{
    //     account::register::{v3::Request as RegistrationRequest, RegistrationKind},
    //     uiaa,
    // },
    TransactionId,
    OwnedRoomId,

    // DeviceId,
    // events::room::message::OriginalSyncRoomMessageEvent,
};


use url::Url;

// mod matrix;
use matrix_goose::matrix::{
    GooseMatrixClient,
    MatrixResponse,
    MatrixError,
    GOOSE_USERS, //GOOSE_USERS_READER,

    config::SyncSettings,
};


#[derive(Debug, serde::Deserialize)]
struct User {
    username: String,
    password: String,
}

struct ClientData {
    room_id: Option<OwnedRoomId>,
    room_tokens: HashMap<OwnedRoomId, String>,
    sync_forever_handle: JoinHandle<()>,
}

static mut USERS: Vec<User> = Vec::new();
static USERS_READER: &Vec<User> = unsafe { &USERS };

// Note that a single goose user reference may required shared ownership between two
// threads (sync_forever and logic thread) depending on the current state of the tokio
// runtime task scheduler.
static mut CLIENTS: Lazy<HashMap<usize, Arc<GooseMatrixClient>>> = Lazy::new(|| { HashMap::new() });

lazy_static! {
    static ref CANCELED: Arc<RwLock<bool>> = Arc::new(RwLock::new(false));
}

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

async fn teardown(user: &mut GooseUser) -> TransactionResult {
    println!("Tearing down loadtest...");

    Ok(())
}

async fn on_start(user: &mut GooseUser) -> TransactionResult {
    let thread_index = user.weighted_users_index;
    unsafe {
        let host = user.base_url.to_owned();
        GOOSE_USERS.push(user);

        let static_client_ref = Arc::new(GooseMatrixClient::new(host, thread_index).await.unwrap());
        CLIENTS.insert(thread_index, static_client_ref);

        let mut client = Arc::clone(&CLIENTS[&thread_index]);
        let csv_user = &USERS_READER[thread_index];
        let username = csv_user.username.to_owned();
        let password = csv_user.password.to_owned();

        match client.login_username(&username, &password).send().await {
            Ok(_) => {
                println!("[{}] Logged in successfully", username);

                // Spawn sync_forever task
                let handle = tokio::spawn(async move {
                    println!("Spawning sync_forever task");
                    let _ = client.sync(SyncSettings::default()).await;
                    println!("[{}] Warning sync_forever future returned early", username);
                });

                user.set_session_data(ClientData { room_id: None, room_tokens: HashMap::new(), sync_forever_handle: handle });

                Ok(())
            },
            Err(error) => {
                println!("[{}] Error logging in: {:?}", username, error);
                Err(error.goose_error.unwrap())
            }
        }
    }
}

async fn on_stop(user: &mut GooseUser) -> TransactionResult {
    let client_data = user.get_session_data::<ClientData>().unwrap();
    client_data.sync_forever_handle.abort();

    Ok(())
}

// Sleep handler that allows for graceful termination so reports can be generated
async fn client_sleep(delay: f64) {
    let mut count = delay;

    // Task sleeping hangs goose termination for long sleep durations, so its needed
    // frequently poll if sleep should be canceled.
    while (count > 1.0) && !*CANCELED.read().await {
        tokio::select! {
            _ = sleep(Duration::from_secs_f64(1.0)) => { count -= 1.0; },
            _ = tokio::signal::ctrl_c() => { *CANCELED.write().await = true; },
        };
    }

    // Finish up remaining sleep time
    if count.fract() > 0.0 {
        sleep(Duration::from_secs_f64(count.fract())).await;
    }
}

async fn do_nothing(_user: &mut GooseUser) -> TransactionResult { Ok(()) }

async fn send_text(user: &mut GooseUser) -> TransactionResult {
    let thread_index = user.weighted_users_index;
    let client = get_client(thread_index).await;
    let username = client.user_id().unwrap().localpart();
    use ruma::api::client::typing::create_typing_event::v3::Request as TypingRequest;
    use ruma::api::client::typing::create_typing_event::v3::Typing as Typing;
    use ruma::api::client::message::send_message_event::v3::Request as MessageRequest;
    use ruma::events::room::message::RoomMessageEventContent as RoomMessage;

    // Send the typing notification like a real client would
    let user_id = client.user_id().unwrap().to_owned();
    let room_id;

    match user.get_session_data::<ClientData>().unwrap().room_id.to_owned() {
        Some(id) => room_id = id.to_owned(),
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
    client_sleep(delay).await;

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
    let thread_index = user.weighted_users_index;
    let client = get_client(thread_index).await;

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
    let thread_index = user.weighted_users_index;
    let client = get_client(thread_index).await;
    let username = client.user_id().unwrap().localpart();

    let room_id;
    match client.joined_rooms().choose(&mut rand::thread_rng()) {
        Some(joined) => room_id = joined.room_id().to_owned(),
        None => return Ok(()),
    }

    let client_data = user.get_session_data_mut::<ClientData>().unwrap();
    client_data.room_id = Some(room_id.to_owned());
    // match user.get_session_data_mut::<ClientData>() {
    //     Some(client_data) => client_data.room_id = room_id.to_owned(),
    //     None => user.set_session_data(
    //         ClientData { room_id: room_id.to_owned(), room_tokens: HashMap::new() }),
    // }

    // Note: consider swapping this with the client API call instead? no need to keep track of tokens

    use ruma::api::client::message::get_message_events::v3::Request as Request;
    use ruma::api::Direction as Direction;


    let mut request = Request::new(room_id.to_owned(), Direction::Backward);
    if let Some(token) = client_data.room_tokens.get(&room_id) {
        request.from = Some(token.to_owned());
    }

    match client.send(request, None).await {
        Ok(response) => {
            // if let Some(token) = response.response.unwrap().end {
            //     client_data.room_tokens.insert(room_id, token);
            // }
        },
        Err(_) => println!("[{}] failed /messages failed for room [{}]", username, room_id),
    }

    Ok(())
}

async fn go_afk(user: &mut GooseUser) -> TransactionResult {
    let thread_index = user.weighted_users_index;

    let csv_user = &USERS_READER[thread_index];
    let username = &csv_user.username.to_owned();
    println!("[{}] going away from keyboard", username);

    // Generate large(ish) random away time.
    let exp = Exp::new(1.0 / 600.0).unwrap(); // Expected value = 10 minutes
    let delay = exp.sample(&mut rand::thread_rng());
    client_sleep(delay).await;

    Ok(())
}

async fn change_displayname(user: &mut GooseUser) -> TransactionResult {
    let thread_index = user.weighted_users_index;
    let client = get_client(thread_index).await;
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
    let thread_index = user.weighted_users_index;
    let client = get_client(thread_index).await;
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
            .register_transaction(transaction!(do_nothing)
                .set_name("Do nothing")
                .set_sequence(1)
                .set_weight(11)?)
            .register_transaction(transaction!(send_text)
                .set_name("Send text")
                .set_sequence(1)
                // .set_weight(1)?)
                .set_weight(3)?)
            .register_transaction(transaction!(look_at_room)
                .set_name("Look at room")
                .set_sequence(1)
                .set_weight(4)?)
            .register_transaction(transaction!(paginate_room)
                .set_name("Paginate room")
                .set_sequence(1)
                .set_weight(1)?)
            .register_transaction(transaction!(go_afk)
                .set_name("Go AFK")
                .set_sequence(1)
                .set_weight(1)?)
            .register_transaction(transaction!(change_displayname)
                .set_name("Change displayname")
                .set_sequence(1)
                .set_weight(1)?)
            .register_transaction(transaction!(on_stop)
                .set_on_stop()
                .set_name("On stop"))
            .set_wait_time(Duration::ZERO, Duration::ZERO)?
        )
        // .register_scenario(scenario!("ChatInARoom")
        //     .register_transaction(transaction!(send_text))
        //     .register_transaction(transaction!(send_image))
        //     .register_transaction(transaction!(send_reaction))
        //     .set_wait_time(Duration::ZERO, Duration::ZERO)?
        // )
        .test_stop(transaction!(teardown))
        .set_default(GooseDefault::HatchRate, "32")?
        .execute()
        .await?;

    Ok(())
}

