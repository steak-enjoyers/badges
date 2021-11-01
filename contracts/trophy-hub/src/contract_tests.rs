use cosmwasm_std::testing::{
    mock_dependencies, mock_env, mock_info, MockApi, MockQuerier, MockStorage,
};
use cosmwasm_std::{
    from_binary, to_binary, Addr, ContractResult, CosmosMsg, Deps, Empty, Event, OwnedDeps, Reply,
    SubMsg, SubMsgExecutionResponse, WasmMsg,
};
use cw721::Expiration;

use serde::de::DeserializeOwned;

use k256::ecdsa::signature::{DigestSigner, Signature};
use k256::ecdsa::{Signature as EcdsaSignature, SigningKey, VerifyingKey};
use rand::rngs::OsRng;
use sha2::{Digest, Sha256};

use terra_trophies::hub::{
    ContractInfoResponse, ExecuteMsg, InstantiateMsg, MintRule, QueryMsg, TrophyInfo,
};
use terra_trophies::legacy::LegacyTrophyInfo;
use terra_trophies::metadata::Metadata;
use terra_trophies::nft::ExecuteMsg as NftExecuteMsg;
use terra_trophies::testing::assert_generic_error_message;

use crate::contract::{execute, instantiate, migrate, query, reply};
use crate::state::State;

// TESTS

#[test]
fn proper_init_hook() {
    let mut deps = mock_dependencies(&[]);
    reply(deps.as_mut(), mock_env(), mock_reply()).unwrap();

    let res_bin = query(deps.as_ref(), mock_env(), QueryMsg::ContractInfo {}).unwrap();
    let res: ContractInfoResponse = from_binary(&res_bin).unwrap();

    let expected = ContractInfoResponse {
        nft: "nft".to_string(),
        trophy_count: 0,
    };
    assert_eq!(res, expected);
}

#[test]
fn proper_instantiation() {
    let mut deps = mock_dependencies(&[]);
    let info = mock_info("deployer", &[]);

    let msg = InstantiateMsg {
        nft_code_id: 123,
    };
    let res = instantiate(deps.as_mut(), mock_env(), info, msg).unwrap();

    let expected = SubMsg::reply_on_success(
        WasmMsg::Instantiate {
            admin: Some("deployer".to_string()),
            code_id: 123,
            msg: to_binary(&Empty {}).unwrap(),
            funds: vec![],
            label: "trophy-nft".to_string(),
        },
        0,
    );
    assert_eq!(res.messages.len(), 1);
    assert_eq!(res.messages[0], expected);
}

#[test]
fn editing_trophy() {
    let mut deps = setup_test();

    // create a trophy
    let msg = ExecuteMsg::CreateTrophy {
        rule: MintRule::ByMinter("creator".to_string()),
        metadata: mock_metadata(),
        expiry: Some(Expiration::AtHeight(20000)),
        max_supply: None,
    };
    execute(deps.as_mut(), mock_env(), mock_info("creator", &[]), msg).unwrap();

    // prepare new metadata
    let mut metadata = mock_metadata();
    metadata.name = Some("Updated Trophy Name".to_string());

    // non-creator can't edit
    let msg = ExecuteMsg::EditTrophy {
        trophy_id: 1,
        metadata,
    };
    let err = execute(deps.as_mut(), mock_env(), mock_info("non-creator", &[]), msg.clone());
    assert_generic_error_message(err, "caller is not creator");

    // creator can edit
    execute(deps.as_mut(), mock_env(), mock_info("creator", &[]), msg).unwrap();

    // metadata should have been updated
    let res: TrophyInfo<String> = query_helper(
        deps.as_ref(),
        QueryMsg::TrophyInfo {
            trophy_id: 1,
        },
    );
    assert_eq!(res.metadata.name, Some("Updated Trophy Name".to_string()));
}

#[test]
fn minting_by_minter() {
    let mut deps = setup_test();

    // first, create the trophy
    // make sure `rule` is set to `ByMinter`
    let msg = ExecuteMsg::CreateTrophy {
        rule: MintRule::ByMinter("minter".to_string()),
        metadata: mock_metadata(),
        expiry: None,
        max_supply: None,
    };
    execute(deps.as_mut(), mock_env(), mock_info("creator", &[]), msg).unwrap();

    // non-minter can't mint
    let msg = ExecuteMsg::MintByMinter {
        trophy_id: 1,
        owners: vec!["alice".to_string(), "bob".to_string()],
    };
    let err = execute(deps.as_mut(), mock_env(), mock_info("non-minter", &[]), msg.clone());
    assert_generic_error_message(err, "caller is not minter");

    // minter can mint
    let res = execute(deps.as_mut(), mock_env(), mock_info("minter", &[]), msg).unwrap();
    let expected = CosmosMsg::Wasm(WasmMsg::Execute {
        contract_addr: "nft".to_string(),
        msg: to_binary(&NftExecuteMsg::Mint {
            trophy_id: 1,
            start_serial: 1,
            owners: vec!["alice".to_string(), "bob".to_string()],
        })
        .unwrap(),
        funds: vec![],
    });
    assert_eq!(res.messages[0].msg, expected);
}

use base64;
#[test]
fn minting_by_signature() {
    // generate 2 signing keys. the public key of sk1 will be used to actually create the trophy
    let sk1 = SigningKey::random(&mut OsRng);
    let sk2 = SigningKey::random(&mut OsRng);

    // generate public key which will be provided to the trophy
    let pk1 = VerifyingKey::from(&sk1);
    let pk1_str = base64::encode(pk1.to_bytes());

    // sign message, which is simply alice's address
    let msg_digest = Sha256::new().chain("alice");

    // sig1 is signed by sk1, which is valid, can be used to claim trophy
    // sig2 is signed by sk2, which is invalid
    let sig1: EcdsaSignature = sk1.sign_digest(msg_digest.clone());
    let sig1_str = base64::encode(sig1.as_bytes());
    let sig2: EcdsaSignature = sk2.sign_digest(msg_digest);
    let sig2_str = base64::encode(sig2.as_bytes());

    // instantaite contract
    let mut deps = setup_test();

    // create trophy
    let msg = ExecuteMsg::CreateTrophy {
        rule: MintRule::BySignature(pk1_str),
        metadata: mock_metadata(),
        expiry: None,
        max_supply: None,
    };
    execute(deps.as_mut(), mock_env(), mock_info("creator", &[]), msg).unwrap();

    // alice mints the trophy using a valid signature; should succeed
    let msg = ExecuteMsg::MintBySignature {
        trophy_id: 1,
        signature: sig1_str,
    };
    let res = execute(deps.as_mut(), mock_env(), mock_info("alice", &[]), msg.clone()).unwrap();

    let expected = CosmosMsg::Wasm(WasmMsg::Execute {
        contract_addr: "nft".to_string(),
        msg: to_binary(&NftExecuteMsg::Mint {
            trophy_id: 1,
            start_serial: 1,
            owners: vec!["alice".to_string()],
        })
        .unwrap(),
        funds: vec![],
    });
    assert_eq!(res.messages.len(), 1);
    assert_eq!(res.messages[0].msg, expected);

    // bob attemps to mint using alice's signature; should fail
    let err = execute(deps.as_mut(), mock_env(), mock_info("bob", &[]), msg);
    assert_generic_error_message(err, "signature verification failed");

    // alice attempts to mint trophy using an invalid signature (signed by sk2 instead of sk1);
    // should fail
    let msg = ExecuteMsg::MintBySignature {
        trophy_id: 1,
        signature: sig2_str,
    };
    let err = execute(deps.as_mut(), mock_env(), mock_info("alice", &[]), msg);
    assert_generic_error_message(err, "signature verification failed");
}

#[test]
fn minting_multiple_times() {
    let mut deps = setup_test();

    // first, create the trophy
    let msg = ExecuteMsg::CreateTrophy {
        rule: MintRule::ByMinter("minter".to_string()),
        metadata: mock_metadata(),
        expiry: Some(Expiration::AtHeight(20000)),
        max_supply: None,
    };
    execute(deps.as_mut(), mock_env(), mock_info("creator", &[]), msg).unwrap();

    // mint for a first time
    let msg = ExecuteMsg::MintByMinter {
        trophy_id: 1,
        owners: vec!["alice".to_string(), "bob".to_string()],
    };
    execute(deps.as_mut(), mock_env(), mock_info("minter", &[]), msg).unwrap();

    // try mint a second time; should correctly `start_serial` as 3
    let msg = ExecuteMsg::MintByMinter {
        trophy_id: 1,
        owners: vec!["charlie".to_string()],
    };
    let res = execute(deps.as_mut(), mock_env(), mock_info("minter", &[]), msg).unwrap();
    let expected = CosmosMsg::Wasm(WasmMsg::Execute {
        contract_addr: "nft".to_string(),
        msg: to_binary(&NftExecuteMsg::Mint {
            trophy_id: 1,
            start_serial: 3,
            owners: vec!["charlie".to_string()],
        })
        .unwrap(),
        funds: vec![],
    });
    assert_eq!(res.messages[0].msg, expected);
}

#[test]
fn minting_assert_rule() {
    let mut deps = setup_test();

    let msg = ExecuteMsg::CreateTrophy {
        rule: MintRule::BySignature("pubkey".to_string()),
        metadata: mock_metadata(),
        expiry: None,
        max_supply: None,
    };
    execute(deps.as_mut(), mock_env(), mock_info("creator", &[]), msg).unwrap();

    // the trophy's minting rule is `BySignature`, but we attempt to mint by minter; should fail
    let msg = ExecuteMsg::MintByMinter {
        trophy_id: 1,
        owners: vec!["charlie".to_string()],
    };
    let err = execute(deps.as_mut(), mock_env(), mock_info("minter", &[]), msg);
    assert_generic_error_message(err, "minting rule is not `ByMinter`");
}

#[test]
fn minting_assert_expiry() {
    let mut deps = setup_test();

    // first, create the trophy
    let msg = ExecuteMsg::CreateTrophy {
        rule: MintRule::ByMinter("minter".to_string()),
        metadata: mock_metadata(),
        expiry: Some(Expiration::AtHeight(10000)), // by default, mock_env has block number 12345
        max_supply: None,
    };
    execute(deps.as_mut(), mock_env(), mock_info("creator", &[]), msg).unwrap();

    // attempt to mint; should fail
    let msg = ExecuteMsg::MintByMinter {
        trophy_id: 1,
        owners: vec!["charlie".to_string()],
    };
    let err = execute(deps.as_mut(), mock_env(), mock_info("minter", &[]), msg);
    assert_generic_error_message(err, "minting time has elapsed");
}

#[test]
fn minting_assert_max_supply() {
    let mut deps = setup_test();

    // first, create the trophy
    let msg = ExecuteMsg::CreateTrophy {
        rule: MintRule::ByMinter("minter".to_string()),
        metadata: mock_metadata(),
        expiry: None,
        max_supply: Some(1),
    };
    execute(deps.as_mut(), mock_env(), mock_info("creator", &[]), msg).unwrap();

    // trophy have a max supply but we attempt to mint 2; should fail
    let msg = ExecuteMsg::MintByMinter {
        trophy_id: 1,
        owners: vec!["alice".to_string(), "bob".to_string()],
    };
    let err = execute(deps.as_mut(), mock_env(), mock_info("minter", &[]), msg);
    assert_generic_error_message(err, "max supply exceeded");
}

#[test]
fn migrating() {
    let mut deps = setup_test();

    let token_id = 1 as u64;
    let state = State::default();

    // create a trophy in legacy format
    let trophy_legacy = LegacyTrophyInfo {
        creator: Addr::unchecked("creator"),
        metadata: mock_metadata(),
        instance_count: 0,
    };
    state.trophies_legacy.save(deps.as_mut().storage, token_id.into(), &trophy_legacy).unwrap();
    state.trophy_count.save(deps.as_mut().storage, &1).unwrap();

    // migrate
    migrate(deps.as_mut(), mock_env(), Empty {}).unwrap();

    // trophy info should have been updated to the current format
    let res: TrophyInfo<String> = query_helper(
        deps.as_ref(),
        QueryMsg::TrophyInfo {
            trophy_id: 1,
        },
    );
    let expected = TrophyInfo {
        creator: "creator".to_string(),
        rule: MintRule::ByMinter("creator".to_string()),
        metadata: mock_metadata(),
        expiry: None,
        max_supply: None,
        current_supply: 0,
    };
    assert_eq!(res, expected);
}

// HELPERS

fn mock_reply() -> Reply {
    let event = Event::new("instantiate_contract").add_attribute("contract_address", "nft");
    let result = ContractResult::Ok(SubMsgExecutionResponse {
        events: vec![event],
        data: None,
    });
    Reply {
        id: 0,
        result,
    }
}

fn mock_metadata() -> Metadata {
    Metadata {
        image: Some("ipfs://image".to_string()),
        image_data: None,
        external_url: None,
        description: Some("This is a test".to_string()),
        name: Some("Test Trophy".to_string()),
        attributes: None,
        background_color: None,
        animation_url: Some("ipfs://video".to_string()),
        youtube_url: None,
    }
}

fn setup_test() -> OwnedDeps<MockStorage, MockApi, MockQuerier> {
    let mut deps = mock_dependencies(&[]);
    reply(deps.as_mut(), mock_env(), mock_reply()).unwrap();
    deps
}

fn query_helper<T: DeserializeOwned>(deps: Deps, msg: QueryMsg) -> T {
    from_binary(&query(deps, mock_env(), msg).unwrap()).unwrap()
}
