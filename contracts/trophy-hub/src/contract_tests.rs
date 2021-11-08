use cosmwasm_std::testing::{
    mock_dependencies, mock_env, mock_info, MockApi, MockQuerier, MockStorage,
};
use cosmwasm_std::{
    from_binary, to_binary, Api, ContractResult, CosmosMsg, Deps, Empty, Event, OwnedDeps, Reply,
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
use terra_trophies::metadata::Metadata;
use terra_trophies::nft::ExecuteMsg as NftExecuteMsg;
use terra_trophies::testing::assert_generic_error_message;

use crate::contract::{execute, instantiate, query, reply};

// TESTS

#[test]
fn verifying_signature() {
    // this is a private key I randomly generated using npm package `secp256k1`
    let sk_str = "lzsKX6ET85qvCozaOEAOmHq9FvjUMr7qNup1sw5Z2MU=";
    let sk_bytes = base64::decode(sk_str).unwrap();
    let sk = SigningKey::from_bytes(&sk_bytes).unwrap();

    // let's first try creating the public key, encode to base64, and see if the result is the same
    // as the one generated by the JS library
    let pk = VerifyingKey::from(&sk);
    let pk_str = base64::encode(pk.to_bytes());
    assert_eq!(&pk_str, "AnGjUHKo/3cbmhJiDlV7ybUsxNeKfDlqtza86Sts7cTk");

    // sign the message, check if it's the same as signed by the JS library
    let msg = "terra1x46rqay4d3cssq8gxxvqz8xt6nwlz4td20k38v";
    let msg_digest = Sha256::new().chain(msg);
    let sig: EcdsaSignature = sk.sign_digest(msg_digest.clone());
    let sig_str = base64::encode(sig.as_bytes());
    assert_eq!(
        sig_str,
        "ag/LE5bQAJlI91QocezC+sk1WBlbPsmonVQf2SDheWsypIOwEZAXtUMh+hQ2bAFYU68UsPAXDcQsw8WeioFkbw=="
    );

    // finally let's verify the signature
    let deps = mock_dependencies(&[]);
    let msg_hash = msg_digest.finalize();
    let verified = deps.api.secp256k1_verify(&msg_hash, &sig.as_bytes(), &pk.to_bytes()).unwrap();
    assert_eq!(verified, true);
}

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
fn minting_by_signature() {
    // generate 2 signing keys. the public key of sk1 will be used to actually create the trophy
    let sk1 = SigningKey::random(&mut OsRng);
    let sk2 = SigningKey::random(&mut OsRng);

    // generate public key which will be provided to the trophy
    let pk1 = VerifyingKey::from(&sk1);
    let pk1_str = base64::encode(pk1.to_bytes());

    // alice properly signs a message using the the correct key (sk1)
    let msg1 = "alice";
    let msg1_digest = Sha256::new().chain(msg1);
    let sig1: EcdsaSignature = sk1.sign_digest(msg1_digest.clone());
    let sig1_str = base64::encode(sig1.as_bytes());

    // bob signs the message using an incorrect key (sk2)
    let msg2 = "bob";
    let msg2_digest = Sha256::new().chain(msg2);
    let sig2: EcdsaSignature = sk2.sign_digest(msg2_digest);
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

    // alice attempts to mint the same trophy a seconds time; should fail
    let err = execute(deps.as_mut(), mock_env(), mock_info("alice", &[]), msg.clone());
    assert_generic_error_message(err, "already minted: alice");

    // bob attempts to mint using alice's signature; should fail
    let err = execute(deps.as_mut(), mock_env(), mock_info("bob", &[]), msg);
    assert_generic_error_message(err, "signature verification failed");

    // bob attempts to mint trophy using an invalid signature (signed by sk2 instead of sk1);
    // should fail
    let msg = ExecuteMsg::MintBySignature {
        trophy_id: 1,
        signature: sig2_str,
    };
    let err = execute(deps.as_mut(), mock_env(), mock_info("bob", &[]), msg);
    assert_generic_error_message(err, "signature verification failed");
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
