use goose::prelude::*;
use std::{
    time::Duration,
    fs::File,
    io::BufReader,
};
use serde::Deserialize;

use matrix_sdk::ruma::{
    api::client::{
        room::{
            create_room::v3::Request as CreateRoomRequest,
        }
    },
};
use ruma_common::{
    UserId, OwnedUserId,
};

use matrix_goose::matrix::{
    GooseMatrixClient,
    GOOSE_USERS,
};


#[derive(Debug, Deserialize)]
struct User {
    username: String,
    password: String,
}

#[derive(Debug, Deserialize)]
struct RoomInfo {
    creator: String,
    name: String,
    users: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct RoomList {
    creators: Vec<RoomInfo>,
}


// For setup tests, only a single thread access its own client
static mut USERS: Vec<User> = Vec::new();
static USERS_READER: &Vec<User> = unsafe { &USERS };

static mut ROOMS: RoomList = RoomList { creators: Vec::new() };
static ROOMS_READER: &RoomList = unsafe { &ROOMS };


async fn setup(user: &mut GooseUser) -> TransactionResult {
    println!("Setting up loadtest...");

    // Load users from csv
    unsafe {
        let num_users = user.config.users.unwrap();

        for _ in 0 .. num_users {
            GOOSE_USERS.push(std::ptr::null_mut());
        }

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

        // Load rooms from csv

        // Open the file in read-only mode with buffer.
        match File::open("rooms.json") {
            Ok(file) => {
                let reader = BufReader::new(file);

                // Read the JSON contents of the file as an instance of `User`.
                match serde_json::from_reader::<_, RoomList>(reader) {
                    Ok(rooms) => {
                        ROOMS = rooms;
                    },
                    Err(err) => panic!("Error reading rooms.json contents: {}", err),
                }
            },
            Err(err) => panic!("Error reading rooms.json: {}", err),
        }
    }

    Ok(())
}

async fn teardown(_user: &mut GooseUser) -> TransactionResult {
    println!("Tearing down loadtest...");

    Ok(())
}


async fn create_room(user: &mut GooseUser) -> TransactionResult {
    let user_index = user.weighted_users_index;

    // Load the next user who needs to be registered
    let csv_user = &USERS_READER[user_index];
    println!("User {}: Got user/pass {} {}", user_index, csv_user.username, csv_user.password);

    // Create matrix client
    let username = &csv_user.username.to_owned();
    let password = &csv_user.password.to_owned();
    let host = user.base_url.to_owned();

    // Populate static table used by matrix API for interfacing with Goose
    unsafe { GOOSE_USERS[user_index] = user };

    if ROOMS_READER.creators.iter().filter(|&room_info| room_info.creator == *username).count() > 0 {
        let client = GooseMatrixClient::new(host, user_index).await.unwrap();

        match client.login_username(username, password).send().await {
            Ok(_) => {
                let rooms_iter = ROOMS_READER.creators.iter().filter(|&room_info| room_info.creator == *username);

                for room_info in rooms_iter {
                    let mut request = CreateRoomRequest::new();
                    let room_name = room_info.name.to_owned();

                    let mut invite_list: Vec<OwnedUserId> = Vec::new();

                    for name in room_info.users.iter() {
                        let host = client.homeserver().await.host_str().unwrap().to_owned().replace("matrix.", "");
                        let full_name = "@".to_owned() + name + ":" + &host;
                        let user_id = <&UserId>::try_from(full_name.as_str()).unwrap().to_owned();

                        // println!("USER ID: {:?}", user_id);
                        invite_list.push(user_id);
                    }

                    request.name = Some(room_name.clone());
                    request.invite = invite_list;

                    let mut retries = 3;

                    // Send request, retry if necessary
                    while retries > 0 {
                        match client.create_room(request.to_owned()).await {
                            Ok(response) => {
                                println!("[{}] Created room {}", username, response.room_id());
                                break;
                            },
                            Err(err) => {
                                println!("[{}] Could not create room {} (attempt {}): {:?}. Trying again...",
                                    username, room_name, 4 - retries, err);
                                retries -= 1;

                                if retries == 0 {
                                    println!("[{}] Error creating room {}. Skipping...", username, room_name);
                                    break;
                                }
                            },
                        }
                    }
                }
            },
            Err(err) => { println!("[{}] Failed login: {:?}", username, err); },
        }
    }

    Ok(())
}



#[tokio::main]
async fn main() -> Result<(), GooseError> {
    println!("Starting matrix user create_room loadtest...");

    // Run test
    GooseAttack::initialize()?
        .test_start(transaction!(setup))
        .register_scenario(scenario!("Create Room")
            .register_transaction(transaction!(create_room))
            .set_wait_time(Duration::ZERO, Duration::ZERO)?
        )
        .test_stop(transaction!(teardown))
        .set_default(GooseDefault::HatchRate, "32")?
        .execute()
        .await?;

    Ok(())
}

