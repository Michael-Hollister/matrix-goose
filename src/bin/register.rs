use goose::prelude::*;
use ruma::api::client::uiaa::Dummy;
use std::time::Duration;

use matrix_sdk::ruma::{
    api::client::{
        account::register::{v3::Request as RegistrationRequest},
        uiaa,
    },
};

use matrix_goose::{
    matrix::{
        GooseMatrixClient,
        GOOSE_USERS,
    },
};


#[derive(Debug, serde::Deserialize)]
struct User {
    username: String,
    password: String,
}

// For setup tests, only a single thread access its own client
static mut USERS: Vec<User> = Vec::new();
static USERS_READER: &Vec<User> = unsafe { &USERS };

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
    }

    Ok(())
}

async fn teardown(_user: &mut GooseUser) -> TransactionResult {
    println!("Tearing down loadtest...");

    Ok(())
}

async fn register(user: &mut GooseUser) -> TransactionResult {
    let user_index = user.weighted_users_index;

    // Load the next user who needs to be registered
    let csv_user = &USERS_READER[user_index];
    println!("User {}: Got user/pass {} {}", user_index, csv_user.username, csv_user.password);

    // Create matrix client
    let username = csv_user.username.to_owned();
    let password = csv_user.password.to_owned();
    let host = user.base_url.to_owned();

    // Populate static table used by matrix API for interfacing with Goose
    unsafe { GOOSE_USERS[user_index] = user };
    let mut request = RegistrationRequest::new();

    request.username = Some(username.to_owned());
    request.password = Some(password.to_owned());
    request.auth = Some(uiaa::AuthData::Dummy(Dummy::new()));

    let client = GooseMatrixClient::new(host, user_index).await.unwrap();
    let mut retries = 3;

    // Send request, retry if necessary
    while retries > 0 {
        match client.register(request.to_owned()).await {
            Ok(_) => {
                println!("[{}] User registration success", username);
                return Ok(());
            },
            Err(err) => {
                println!("[{}] Could not register user (attempt {}): {:?}. Trying again...",
                    username, 4 - retries, err);
                retries -= 1;
            },
        }
    }

    println!("Error registering user {}. Skipping...", username);

    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), GooseError> {
    println!("Starting matrix user register loadtest...");

    // Run test
    GooseAttack::initialize()?
        .test_start(transaction!(setup))
        .register_scenario(scenario!("Register")
            .register_transaction(transaction!(register))
            .set_wait_time(Duration::ZERO, Duration::ZERO)?
        )
        .test_stop(transaction!(teardown))
        .set_default(GooseDefault::HatchRate, "32")?
        .execute()
        .await?;

    Ok(())
}
