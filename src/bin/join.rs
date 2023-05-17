use goose::prelude::*;
use std::time::Duration;

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

// For setup tests, only a single thread access its own client
static mut USERS: Vec<Vec<User>> = Vec::new();
static USERS_READER: &Vec<Vec<User>> = unsafe { &USERS };


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
    }

    Ok(())
}

async fn teardown(user: &mut GooseUser) -> TransactionResult {
    println!("Tearing down loadtest...");

    Ok(())
}


async fn join(user: &mut GooseUser) -> TransactionResult {
    let thread_index = user.weighted_users_index;
    let user_ctr = user.get_iterations();

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

        let client = GooseMatrixClient::new(host, thread_index).await.unwrap();

        println!("Making login request for {}", username);
        if let response1 = client.login_username(username, password).send().await {
            client.sync_once(SyncSettings::default()).await;

            for invite in client.invited_rooms() {
                client.join_room_by_id(invite.room_id()).await;
            }

        }
        else {
            println!("Failed login for {}", username);
        }


        // logging.info("User [%s] has %d pending invites", self.matrix_client.user, len(invited_rooms))
        // for room_id in invited_rooms:
        //     retries = 3
        //     while retries > 0:
        //         response = self.matrix_client.join(room_id)

        //         if isinstance(response, JoinError):
        //             logging.error("[%s] Could not join room %s (attempt %d). Trying again...",
        //                          self.matrix_client.user, room_id, 4 - retries)
        //             logging.error("[%s] Code=%s, Message=%s", self.matrix_client.user,
        //                           response.status_code, response.message)
        //             retries -= 1
        //         else:
        //             logging.info("[%s] Joined room %s", self.matrix_client.user, room_id)
        //             break

        //     if retries == 0:
        //         logging.error("[%s] Error joining room %s. Skipping...", self.matrix_client.user, room_id)

    }
    else {
        // println!("out of users");
    }


    Ok(())
}



#[tokio::main]
async fn main() -> Result<(), GooseError> {
    println!("Starting matrix user join loadtest...");

    // Run test
    GooseAttack::initialize()?
        .test_start(transaction!(setup))
        .register_scenario(scenario!("Join")
            .register_transaction(transaction!(join))
            .set_wait_time(Duration::ZERO, Duration::ZERO)?
        )
        .test_stop(transaction!(teardown))
        .set_default(GooseDefault::HatchRate, "32")?
        .execute()
        .await?;

    Ok(())
}

