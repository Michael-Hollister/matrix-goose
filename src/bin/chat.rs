use goose::prelude::*;
use rand::Rng;
use rand::seq::SliceRandom;

use ruma_common::serde::Raw;
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
use serde_json::{json, value::to_raw_value};

// use matrix_sdk::Client;
use matrix_sdk::{
    ruma::{
        // events::room::message::SyncRoomMessageEvent,
        events::room::message::OriginalSyncRoomMessageEvent,
        TransactionId,
        OwnedRoomId,
        OwnedUserId,
    },
};

use matrix_goose::{
    matrix::{
        GooseMatrixClient,
        GOOSE_USERS,

        config::SyncSettings,
        room::Room,
    },
    task_sleep, CANCELED,
};


#[derive(Debug, Clone, serde::Deserialize)]
struct User {
    username: String,
    password: String,
}

// TODO: Switch to using the client store instead of user session data
#[derive(Debug)]
struct ClientData {
    room_id: Option<OwnedRoomId>,
    room_tokens: HashMap<OwnedRoomId, String>,
    room_messages: HashMap<OwnedRoomId, Vec<OriginalSyncRoomMessageEvent>>,
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
                client.add_event_handler(on_room_message);

                // Replicate `sync` method behavior from the SDK client
                let mut last_sync_time: Option<std::time::Instant> = None;

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

                        // Replicate `sync` method behavior from the SDK client
                        GooseMatrixClient::delay_sync(&mut last_sync_time).await
                    }
                });

                user.set_session_data(ClientData {
                    room_id: None,
                    room_tokens: HashMap::new(),
                    room_messages: HashMap::new(),
                    sync_forever_handle: handle }
                );

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
    let index_weights = [11, 6, 4, 2, 1, 1, 0, 1];
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

async fn on_room_message(event: OriginalSyncRoomMessageEvent, room: Room, client: GooseMatrixClient) {
    // Consider protecting with RwLock, or enforce sync and user tasks run only on the same thread
    let user: &mut GooseUser = unsafe { GOOSE_USERS[client.inner.goose_user_index].as_mut().unwrap() };
    let client_data = user.get_session_data_mut::<ClientData>().unwrap();
    // println!("Got message '{}' in room {}", event.content.body(), room.room_id());

    // Add the new messages to whatever we had before (if anything)
    match client_data.room_messages.get_mut(&room.room_id().to_owned()) {
        Some(messages) => {
            messages.push(event);
        },
        None => {
            let mut room_messages = Vec::new();
            room_messages.push(event.to_owned());
            client_data.room_messages.insert(room.room_id().to_owned(), room_messages);
        },
    }
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
    use ruma::api::client::message::send_message_event::v3::Request as MessageRequest;
    use ruma::events::room::message::RoomMessageEventContent as RoomMessage;

    // Send the typing notification like a real client would
    let room_id;
    match user.get_session_data::<ClientData>().unwrap().room_id.to_owned() {
        Some(id) => {room_id = id.to_owned(); },
        None => return Ok(()),
    }

    if client.get_joined_room(&room_id).unwrap().typing_notice(true).await.is_err() {
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
    let client_data = user.get_session_data::<ClientData>().unwrap();
    let username = client.user_id().unwrap().localpart();
    use ruma::api::client::receipt::create_receipt::v3::ReceiptType as ReceiptType;
    use ruma_common::events::receipt::ReceiptThread as ReceiptThread;

    let room_id;
    match client.joined_rooms().choose(&mut rand::thread_rng()) {
        Some(joined) => room_id = joined.room_id().to_owned(),
        None => return Ok(()),
    }

    // println!("[{}] Looking at room [{}]", username, room_id);

    // load_data_for_room
    //     # FIXME Need to parse the room state for all of this :-\
    //     ## FIXME Load the room displayname and avatar url
    //     ## FIXME If we don't have it, load the avatar image
    //     #room_displayname = self.room_display_names.get(room_id, None)
    //     #if room_displayname is None:
    //     #  # Uh-oh, do we need to parse the room state from /sync in order to get this???
    //     #  pass
    //     #room_avatar_url = self.room_avatar_urls.get(room_id, None)
    //     #if room_avatar_url is None:
    //     #  # Uh-oh, do we need to parse the room state from /sync in order to get this???
    //     #  pass
    //     ## Note: We may have just set room_avatar_url in the code above
    //     #if room_avatar_url is not None and self.media_cache.get(room_avatar_url, False) is False:
    //     #  # FIXME Download the image and set the cache to True
    //     #  pass

    // Load the avatars for recent users
    // Load the thumbnails for any messages that have one
    // for message in client_data.recent_messages.get(&room_id).into_iter().flatten() {
    //     // let sender_user_id = message.sender;
    //     // let sender_avatar_mxc = client_data.user_avatar_urls.get(&message.sender);
    //     // client.get_profile(user_id)

    //     if let Ok(Some(profile)) = client.store().get_profile(&room_id, &message.sender).await {
    //         if let Some(state_event) = profile.as_original() {
    //             // Get avatar URL and download if necessary
    //             if state_event.content.avatar_url.is_none() {

    //             }
    //         }
    //         // download avatar / get url
    //         client.store()
    //     }


    // sender_userid = message.sender
    // sender_avatar_mxc = self.user_avatar_urls.get(sender_userid, None)
    // if sender_avatar_mxc is None:
    //     # FIXME Fetch the avatar URL for sender_userid
    //     # FIXME Set avatar_mxc
    //     # FIXME Set self.user_avatar_urls[sender_userid]
    //     self.matrix_client.get_avatar(sender_userid)
    // # Try again.  Maybe we were able to populate the cache in the line above.
    // sender_avatar_mxc = self.user_avatar_urls.get(sender_userid, None)
    // # Now avatar_mxc might not be None, even if it was above
    // if sender_avatar_mxc is not None and len(sender_avatar_mxc) > 0:
    //     # FIXME Reimplement method with nio after avatar support is added
    //     self.download_matrix_media(sender_avatar_mxc)
    // sender_displayname = self.user_display_names.get(sender_userid, None)
    // if sender_displayname is None:
    //     sender_displayname = self.matrix_client.get_displayname(sender_userid)

    // }

    //     # Currently users only send text messages
    //     # for message in messages:
    //     #     content = message.content
    //     #     msgtype = content.msgtype
    //     #     if msgtype in ["m.image", "m.video", "m.file"]:
    //     #         thumb_mxc = message.content.get("thumbnail_url", None)
    //     #         if thumb_mxc is not None:
    //     #             self.download_matrix_media(thumb_mxc)


    if let Some(events) = client_data.room_messages.get(&room_id) {
        let event_id = events.last().unwrap().event_id.to_owned();
        if client.get_joined_room(&room_id).unwrap().send_single_receipt(ReceiptType::Read, ReceiptThread::Unthreaded, event_id).await.is_err() {
            println!("[{}] failed to update read marker in room [{}]", username, room_id);
        }
    }

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

    let user_number = *username.split(".").collect::<Vec<&str>>().last().unwrap();
    let random_number = rand::thread_rng().gen_range(1 .. 1000);
    let new_name = format!("User {} (random={})", user_number, random_number);

    if client.account().set_display_name(Some(&new_name)).await.is_err() {
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
    let client_data = user.get_session_data::<ClientData>().unwrap();
    let username = client.user_id().unwrap().localpart();
    use ruma::api::client::message::send_message_event::v3::Request as MessageRequest;
    use ruma_common::events::MessageLikeEventType as MessageLikeEventType;

    // Pick a recent message from the selected room, and react to it
    let room_id;
    match client.joined_rooms().choose(&mut rand::thread_rng()) {
        Some(joined) => room_id = joined.room_id().to_owned(),
        None => return Ok(()),
    }

    if let Some(messages) = client_data.room_messages.get(&room_id) {
        let slice_start = if messages.len() > 10 { messages.len() - 11 } else { 0 };
        let message = messages[slice_start ..].choose(&mut rand::thread_rng()).unwrap();
        let reaction = ["💩","👍","❤️", "👎", "🤯", "😱", "👏"].choose(&mut rand::thread_rng());
        let content = to_raw_value(&json!({
            "m.relates_to": {
                "rel_type": "m.annotation",
                "event_id": message.event_id,
                "key": reaction,
            }
        })).unwrap();

        // # Prevent errors with reacting to the same message with the same reaction
        // if (message, reaction) in self.reacted_messages:
        //     return
        // else:
        //     self.reacted_messages.append((message, reaction))

        let request = MessageRequest::new_raw(room_id.to_owned(), TransactionId::new(), MessageLikeEventType::Reaction, Raw::from_json(content));
        if client.send(request, None).await.is_err() {
            println!("[{}] failed to send reaction in room [{}]", username, room_id);
        }
    }

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

