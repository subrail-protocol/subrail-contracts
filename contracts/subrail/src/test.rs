#![cfg(test)]

use soroban_sdk::{testutils::Address as _, Address, Env};

use crate::{Error, SubRailContract, SubRailContractClient};

fn setup() -> (Env, SubRailContractClient<'static>) {
    let env = Env::default();
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let contract_id = env.register(SubRailContract, ());
    let client = SubRailContractClient::new(&env, &contract_id);
    client.initialize(&admin);
    (env, client)
}

#[test]
fn initialize_only_once() {
    let (env, client) = setup();
    let again = Address::generate(&env);
    assert_eq!(
        client.try_initialize(&again),
        Err(Ok(Error::AlreadyInitialized))
    );
}

#[test]
fn get_version_returns_1() {
    let (_env, client) = setup();
    assert_eq!(client.get_version(), 1);
}
