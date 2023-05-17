use goose::prelude::*;
use std::time::Duration;

use serde::Deserialize;

use matrix_sdk::ruma::{
    api::client::{
        // account::register::{v3::Request as RegistrationRequest, RegistrationKind},
        // uiaa,
        room::{
            create_room::v3::Request as CreateRoomRequest,
            // Visibility,
        }
    },
    // DeviceId,

};

use ruma_common::{
    UserId, OwnedUserId,
};


// use ruma::user_id

use url::Url;

// use std::error::Error;
use std::fs::File;
use std::io::BufReader;
// use std::path::Path;

// mod matrix;
use matrix_goose::matrix::{
    GooseMatrixClient,
    MatrixResponse,
    MatrixError,
    GOOSE_USERS, //GOOSE_USERS_READER,
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
static mut USERS: Vec<Vec<User>> = Vec::new();
static USERS_READER: &Vec<Vec<User>> = unsafe { &USERS };

static mut ROOMS: RoomList = RoomList { creators: Vec::new() };
static ROOMS_READER: &RoomList = unsafe { &ROOMS };


async fn setup(user: &mut GooseUser) -> TransactionResult {
    println!("Setting up loadtest...");

    // Load users from csv
    unsafe {
        let num_users = user.config.users.unwrap();

        for _ in 0 .. num_users {
            USERS.push(Vec::new());
            GOOSE_USERS.push(std::ptr::null_mut());
        }

        match csv::Reader::from_path("users.csv") {
            Ok(mut reader) => {
                let mut user_count = 0;

                for entry in reader.deserialize::<User>() {
                    match entry {
                        Ok(record) => {
                            // println!("{:?}", record);
                            USERS[user_count % num_users].push(record);
                            user_count += 1;
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

async fn teardown(user: &mut GooseUser) -> TransactionResult {
    println!("Tearing down loadtest...");

    Ok(())
}


async fn create_room(user: &mut GooseUser) -> TransactionResult {
    let thread_index = user.weighted_users_index;
    let user_ctr = user.get_iterations();

    // println!("Goose iterations: {}", user.get_iterations());

    // Load the next user who needs to be registered
    if user_ctr < USERS_READER[thread_index].len() {
        let csv_user = &USERS_READER[thread_index][user_ctr];
        println!("Thread {}: Got user/pass {} {}", thread_index, csv_user.username, csv_user.password);

        // Create matrix client
        let username = &csv_user.username.to_owned();
        let password = &csv_user.password.to_owned();
        let host = user.base_url.to_owned();


        // Populate static table used by matrix API for interfacing with Goose
        unsafe { GOOSE_USERS[thread_index] = user };


        // let client = GooseMatrixClient::new(test_url, user).await.unwrap();
        if ROOMS_READER.creators.iter().filter(|&room_info| room_info.creator == *username).count() > 0 {
            let client = GooseMatrixClient::new(host, thread_index).await.unwrap();

            println!("Making login request for {}", username);
            if let response1 = client.login_username(username, password).send().await {
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

                    // request.name = Some(room_name);

                    request.name = Some(room_name.clone());
                    request.invite = invite_list;
                    // request.invite = invite_list.to_owned();

                    // println!("Making room creation request for {} with room name {} and invite list {:?}", username, room_name, invite_list);
                    println!("Making room creation request for {} with room name {}", username, room_name);
                    // println!("Session token? {:?}", client.access_token());
                    let response2 = client.create_room(request).await;
                    println!("Response: {}", response2.unwrap().room_id() );
                }
            }
            else {
                println!("Failed login for {}", username);
            }
        }
        else {
            println!("No rooms to create for user {}", username);
        }


        //     # Actually create the room
        //     retries = 3
        //     while retries > 0:
        //         response = self.matrix_client.room_create(alias=None, name=room_name, invite=user_ids)

        //         if isinstance(response, RoomCreateError):
        //             logging.error("[%s] Could not create room %s (attempt %d). Trying again...",
        //                          self.matrix_client.user, room_name, 4 - retries)
        //             logging.error("[%s] Code=%s, Message=%s", self.matrix_client.user,
        //                           response.status_code, response.message)
        //             retries -= 1
        //         else:
        //             logging.info("[%s] Created room [%s]", self.matrix_client.user, response.room_id)
        //             break

        //     if retries == 0:
        //         logging.error("[%s] Error creating room %s. Skipping...",
        //                       self.matrix_client.user, room_name)


    }
    else {
        // println!("out of users");
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

