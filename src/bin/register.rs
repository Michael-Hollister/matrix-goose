use goose::prelude::*;
use ruma::api::client::uiaa::Dummy;
use std::time::Duration;
// use genawaiter::{yield_, rc::gen, rc::Gen, GeneratorState};

use matrix_sdk::ruma::{
    api::client::{
        account::register::{v3::Request as RegistrationRequest},
        uiaa,
    },
    DeviceId,
};
use url::Url;

// mod matrix;
use matrix_goose::matrix::{
    GooseMatrixClient,
    MatrixResponse,
    MatrixError,
    GOOSE_USERS, //GOOSE_USERS_READER,
};


#[derive(Debug, serde::Deserialize)]
struct User {
    username: String,
    password: String,
}

// For setup tests, only a single thread access its own client
static mut USERS: Vec<Vec<User>> = Vec::new();
static USERS_READER: &Vec<Vec<User>> = unsafe { &USERS };

// static mut USERS: Vec<genawaiter::rc::Gen<genawaiter::rc::Gen::, R, F>> = Vec::new();
// static USERS_READER: &Vec<genawaiter::rc::Gen<User>> = unsafe { &USERS };


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

async fn register(user: &mut GooseUser) -> TransactionResult {
    // let _goose_metrics = user.get("").await?;
    // user.get(path)

    let thread_index = user.weighted_users_index;
    let user_ctr = user.get_iterations();

    // println!("GOOSE ITERS: {}", user.get_iterations());

    // Load the next user who needs to be registered
    if user_ctr < USERS_READER[thread_index].len() {
        let csv_user = &USERS_READER[thread_index][user_ctr];
        println!("Thread {}: Got user/pass {} {}", thread_index, csv_user.username, csv_user.password);

        // Create matrix client
        let username = csv_user.username.to_owned();
        let password = csv_user.password.to_owned();
        let host = user.base_url.to_owned();

        // Populate static table used by matrix API for interfacing with Goose
        unsafe { GOOSE_USERS[thread_index] = user };
        let mut request = RegistrationRequest::new();

        request.username = Some(username);
        request.password = Some(password);
        request.auth = Some(uiaa::AuthData::Dummy(Dummy::new()));

        let client = GooseMatrixClient::new(host, thread_index).await.unwrap();
        let response = client.register(request).await;


        // retries = 3
        // while retries > 0:
        //     # Register with the server to get a user_id and access_token
        //     response = self.matrix_client.register(self.matrix_client.user, self.matrix_client.password)

        //     if isinstance(response, RegisterErrorResponse):
        //         logging.info("[%s] Could not register user (attempt %d). Trying again...",
        //                      self.matrix_client.user, 4 - retries)
        //         retries -= 1
        //         continue

        //     return

        // logging.error("Error registering user %s. Skipping...", self.matrix_client.user)

    }
    else {
        // println!("out of users");
    }


    // let mut user_generator = gen!({
    // // let user_generator = genawaiter::rc::Gen::new()
    //     println!("re-entering generator");
    //     for entry in USERS_READER[user_reader_thread].iter() {
    //         yield_!(entry);
    //     }
    // });

    // match user_generator.resume() {
    //     GeneratorState::Yielded(csv_user) => {
    //         println!("Thread {}: Got user/pass {} {}", user.weighted_users_index, csv_user.username, csv_user.password);

    //     },
    //     _ => {
    //         println!("out of users");
    //     }
    // }


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

