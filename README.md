# Matrix load generation with Goose

This project provides scripts for load testing [Matrix](https://matrix.org/)
servers using [Goose](https://docs.rs/goose/latest/goose/).

## Getting started

### Prerequisites

We assume that you already have your Matrix homeserver installed and
configured for testing.

* Your homeserver should be configured to allow registering new accounts
  without any kind of email verification or CAPTCHA etc.

* Either turn off rate limiting entirely, or increase your rate limits
  to allow the volume of traffic that you plan to produce, with some
  extra headroom just in case.

If you need help creating a reproducible configuration for your server,
have a look at [matrix-docker-ansible-deploy](https://github.com/spantaleev/matrix-docker-ansible-deploy)
for an Ansible playbook that can be used to install and configure Synapse,
Dendrite, and Conduit, along with any required databases and other
homeserver accessories.

### Generating users and rooms

Before you can use the Goose scripts to load your Matrix server, you
first need to generate usernames and passwords for your Goose users,
as well as the set of rooms where they will chat with each other.

First we generate the usernames and passwords.

```console
[user@host matrix-goose]$ python3 generate_users.py
```

This generates 1000 users by default and saves the usernames and passwords to
a file called `users.csv`. You can also pass in a number to specify the number
of users to generate.

Next we need to decide what the rooms are going to look like in our test.
The `generate_rooms.py` script generates as many rooms as there are users
in `users.csv`.

```console
[user@host matrix-goose]$ python3 generate_rooms.py
```

The script decides how many users should be in each room according to an "80/20"
rule (aka a power law distribution), in an attempt to match real-world
human behavior.
Most rooms will be small -- only 2 or 3 users -- but there is a good
chance that there will be some room so big as to contain every single
user in the test.
Once the script has decided how big each room should be, it selects users
randomly from the population to fill up each room.
It saves the room names and the user-room assignments in the file `rooms.json`.

## Running the tests

The following examples show just a few things that we can do with Goose.

In fact, the user registration script and the room creation script (1 and 2 below)
were not originally intended to stress the server.

You may need to play around with the total number of users and the hatch rate
to find a configuration that your homeserver can handle.

0. Set your homeserver URL
```console
[user@host matrix-goose]$ export HOMESERVER=https://your.homeserver.tld
```

1. Registering user accounts

```console
[user@host matrix-goose]$ cargo run --bin register --release -- --host $HOMESERVER --report-file=register.html --users 1000 --iterations 1 --hatch-rate 10
```

2. Creating rooms

```console
[user@host matrix-goose]$ cargo run --bin create_room --release -- --host $HOMESERVER --report-file=create_room.html --users 1000 --iterations 1 --hatch-rate 10
```

3. Accepting invites to join rooms

```console
[user@host matrix-goose]$ cargo run --bin join --release -- --host $HOMESERVER --report-file=join.html --users 1000 --iterations 1 --hatch-rate 10
```

4. Normal chat activity -- Accepting any pending invites, sending messages, paginating rooms

```console
[user@host matrix-goose]$ cargo run --bin chat --release -- --host $HOMESERVER --report-file=chat.html --no-reset-metrics --users 1000 --hatch-rate 10
```

Note that you also have the ability to modify parameters at runtime. See the
[Controllers](https://book.goose.rs/controller/overview.html) documentation
for more information.

## Running automated tests [Not ported to Goose yet]

This repository supports the ability to run automated tests. You can define
test suites, which are JSON files that describes a series of tests along with
its Locust parameters to run. Examples of test suites are provided in the
`test-suites` directory.

There are also utility scripts (located in the `scripts` directory), but note
that some of these scripts are dependent on a specific server setup.

Example for running a test-suite:

```console
$ python3 run.py --host YOUR_HOMESERVER test-suites/synapse-2k.json
```

Note: For the automation scripts provided in this repository, you should not
prefix the host argument with `https://`.

## Writing your own tests

The base script for interacting with a Matrix homeserver and generating traffic
like a real user is [chat.rs](./src/bin/chat.rs).
