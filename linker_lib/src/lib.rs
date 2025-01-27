/*
    This file is part of TON OS.

    TON OS is free software: you can redistribute it and/or modify
    it under the terms of the Apache License 2.0 (http://www.apache.org/licenses/)

    Copyright 2019-2021 (c) TON LABS
*/

/*

    TODO:
        - move [pyfunction] code to separate file

*/

extern crate base64;
extern crate ed25519_dalek;
extern crate hex;
#[macro_use]
extern crate lazy_static;
extern crate num;
extern crate rand;
// #[macro_use]
extern crate serde_json;

extern crate ton_block;
extern crate ton_types;
#[macro_use]
extern crate ton_vm;
extern crate ton_abi;

mod printer;
mod util;
mod abi;
mod debug_info;
mod global_state;
mod exec;
mod call_contract;
mod messages;

use global_state::{
    GlobalState, GLOBAL_STATE,
};

use util::{
    decode_address, load_from_file,
};

use messages::{
    MessageInfo2,
};

use exec::{
    exec_contract_and_process_actions,
    dispatch_message_impl,
    deploy_contract_impl,
    call_contract_impl,
    load_state_init,
};

use serde_json::Value as JsonValue;

use ed25519_dalek::{
    Keypair, Signer,
};

use rand::rngs::OsRng;

use pyo3::prelude::*;
use pyo3::wrap_pyfunction;
use pyo3::exceptions::PyRuntimeError;

use std::io::Cursor;

use ton_types::{
    SliceData,
    serialize_toc,
    cells_serialization::{deserialize_cells_tree},
};


#[pyfunction]
fn set_trace(trace: bool) -> PyResult<()> {
    GLOBAL_STATE.lock().unwrap().trace = trace;
    Ok(())
}

#[pyfunction]
fn trace_on() -> PyResult<()> {
    GLOBAL_STATE.lock().unwrap().trace_on = true;
    Ok(())
}

#[pyfunction]
fn deploy_contract(
    contract_file: String,
    abi_file: String,
    ctor_params: Option<String>,
    pubkey: Option<String>,
    private_key: Option<String>,
    wc: i8,
    override_address: Option<String>,
    balance: u64,
) -> PyResult<String> {
    let mut gs = GLOBAL_STATE.lock().unwrap();
    let trace = gs.trace;

    let abi_info = gs.all_abis.from_file(&abi_file);

    let state_init = load_state_init(
        &mut gs,
        &contract_file,
        &abi_file,
        &abi_info,
        &ctor_params,
        &pubkey,
        &private_key,
        trace,
    ).map_err(|e| PyRuntimeError::new_err(e))?;

    let target_address = override_address.map(|addr| decode_address(&addr));
    deploy_contract_impl(
        &mut gs,
        Some(contract_file),
        state_init,
        target_address,
        abi_info,
        wc,
        balance,
    ).map_err(|err_str| PyRuntimeError::new_err(err_str))
}

#[pyfunction]
fn get_balance(address: String) -> PyResult<Option<u64>> {
    let address = decode_address(&address);
    let gs = GLOBAL_STATE.lock().unwrap();
    let contract = gs.get_contract(&address);
    let balance = if gs.dummy_balances.contains_key(&address) {
        assert!(contract.is_none());
        Some(gs.dummy_balances[&address])
    } else {
        contract.map(|c| c.balance())
    };
    Ok(balance)
}

#[pyfunction]
fn set_balance(address: String, balance: u64) -> PyResult<()> {
    let address = decode_address(&address);
    let mut gs = GLOBAL_STATE.lock().unwrap();
    let mut contract_info = gs.get_contract(&address).unwrap();
    contract_info.set_balance(balance);
    gs.set_contract(address, contract_info);
    Ok(())
}

#[pyfunction]
fn dispatch_message(msg_id: u32) -> PyResult<(i32, Vec<String>, i64, Option<String>)> {
    let mut gs = GLOBAL_STATE.lock().unwrap();
    let result = dispatch_message_impl(&mut gs, msg_id);
    gs.last_trace = result.trace.clone();
    Ok(result.unpack())
}

#[pyfunction]
fn set_contract_abi(address_str: Option<String>, abi_file: String) -> PyResult<()> {
    let mut gs = GLOBAL_STATE.lock().unwrap();
    let abi_info = gs.all_abis.from_file(&abi_file);
    if let Some(address_str) = address_str {
        let addr = decode_address(&address_str);
        let mut contract_info = gs.get_contract(&addr).unwrap();
        contract_info.set_abi(abi_info);
        gs.set_contract(addr, contract_info);
    }
    Ok(())
}

#[pyfunction]
fn call_ticktock(
    address_str: String,
    is_tock: bool,
) -> PyResult<(i32, Vec<String>, i64, Option<String>)> {
    let address = decode_address(&address_str);

    let mut gs = GLOBAL_STATE.lock().unwrap();
    // TODO: move to call_ticktock_impl()
    let msg_info = MessageInfo2::with_ticktock(is_tock, address.clone());

    let result = exec_contract_and_process_actions(
        &mut gs,
        &msg_info,
        None, // method
    );

    // TODO: register in gs.messages?

    Ok(result.unpack())
}

#[pyfunction]
fn log_str(
    msg: String,
) -> PyResult<()> {

    let mut gs = GLOBAL_STATE.lock().unwrap();
    gs.log_str(msg);

    Ok(())
}

#[pyfunction]
fn call_contract(
    address_str: String,
    method: String,
    is_getter: bool,
    params: String,
    private_key: Option<String>,
) -> PyResult<(i32, Vec<String>, i64, Option<String>)> {
    let mut gs = GLOBAL_STATE.lock().unwrap();
    let result =
        call_contract_impl(&mut gs, address_str, method,
                           is_getter, params, private_key);
    if let Ok(ref result) = result {
        gs.last_trace = result.trace.clone();
    }
    let result = result.map_err(|e| PyRuntimeError::new_err(e))?;
    Ok(result.unpack())
}

// ---------------------------------------------------------------------------------------

#[pyfunction]
fn set_now(now: u64) -> PyResult<()> {
    let mut gs = GLOBAL_STATE.lock().unwrap();
    gs.set_now(now);
    Ok(())
}

#[pyfunction]
fn set_config_param(idx: u32, cell: String) -> PyResult<()> {
    let mut gs = GLOBAL_STATE.lock().unwrap();

    let cell = base64::decode(&cell).unwrap();
    let mut csor = Cursor::new(cell);
    let cell = deserialize_cells_tree(&mut csor).unwrap().remove(0);

    let is_empty = cell.bit_length() == 0;
    if gs.trace {
        println!("set_config_param {} is_empty={}", idx, is_empty);
    }
    if is_empty {
        gs.config_params.remove(&idx);
    } else {
        gs.config_params.insert(idx, cell);
    }

    Ok(())
}

#[pyfunction]
fn reset_all() -> PyResult<()> {
    use std::ops::DerefMut;
    let mut gs = GLOBAL_STATE.lock().unwrap();
    *gs.deref_mut() = GlobalState::default();
    Ok(())
}

#[pyfunction]
fn make_keypair() -> PyResult<(String, String)> {
    let mut csprng = OsRng{};
    let keypair = Keypair::generate(&mut csprng);
    let secret = keypair.to_bytes();
    let secret = hex::encode(secret.to_vec());
    let public = hex::encode(keypair.public.to_bytes());
    Ok((secret, public))
}

#[pyfunction]
fn sign_cell(cell: String, secret: String) -> PyResult<String> {
    let cell = base64::decode(&cell).unwrap();
    // TODO: util?
    let mut csor = Cursor::new(cell);
    let cell = deserialize_cells_tree(&mut csor).unwrap().remove(0);

    let secret = hex::decode(secret).unwrap();
    let keypair = Keypair::from_bytes(&secret).expect("error: invalid key");

    let data = SliceData::from(cell).get_bytestring(0);
    let signature = keypair.sign(&data).to_bytes();
    let signature = hex::encode(signature.to_vec());

    Ok(signature)
}

#[pyfunction]
fn get_all_runs() -> PyResult<String> {
    let gs = GLOBAL_STATE.lock().unwrap();
    let result = serde_json::to_string(&gs.runs).unwrap();
    Ok(result)
}

#[pyfunction]
fn get_all_messages() -> PyResult<String> {
    let gs = GLOBAL_STATE.lock().unwrap();
    let jsons: JsonValue = gs.messages.to_json();
    let result = serde_json::to_string(&jsons).unwrap();
    Ok(result)
}

#[pyfunction]
fn get_last_trace() -> PyResult<String> {
    let gs = GLOBAL_STATE.lock().unwrap();
    let result = serde_json::to_string(&gs.last_trace).unwrap();
    Ok(result)
}

#[pyfunction]
fn load_code_cell(filename: String) -> PyResult<String> {
    let state_init = load_from_file(&filename);
    let code = state_init.code.unwrap();
    let bytes = serialize_toc(&code).unwrap();
    Ok(base64::encode(&bytes))
}

#[pyfunction]
fn load_data_cell(filename: String) -> PyResult<String> {
    // TODO: add tests for that
    let state_init = load_from_file(&filename);
    let data = state_init.data.unwrap();
    let bytes = serialize_toc(&data).unwrap();
    Ok(base64::encode(&bytes))
}

/////////////////////////////////////////////////////////////////////////////////////
/// A Python module implemented in Rust.
#[pymodule]
fn linker_lib(_py: Python, m: &PyModule) -> PyResult<()> {
    m.add_wrapped(wrap_pyfunction!(reset_all))?;

    m.add_wrapped(wrap_pyfunction!(deploy_contract))?;
    m.add_wrapped(wrap_pyfunction!(call_contract))?;
    m.add_wrapped(wrap_pyfunction!(call_ticktock))?;
    m.add_wrapped(wrap_pyfunction!(log_str))?;
    m.add_wrapped(wrap_pyfunction!(get_balance))?;
    m.add_wrapped(wrap_pyfunction!(set_balance))?;

    m.add_wrapped(wrap_pyfunction!(dispatch_message))?;

    m.add_wrapped(wrap_pyfunction!(set_now))?;
    m.add_wrapped(wrap_pyfunction!(set_trace))?;
    m.add_wrapped(wrap_pyfunction!(trace_on))?;
    m.add_wrapped(wrap_pyfunction!(set_contract_abi))?;
    m.add_wrapped(wrap_pyfunction!(set_config_param))?;
    
    m.add_wrapped(wrap_pyfunction!(make_keypair))?;
    m.add_wrapped(wrap_pyfunction!(sign_cell))?;
    m.add_wrapped(wrap_pyfunction!(load_code_cell))?;
    m.add_wrapped(wrap_pyfunction!(load_data_cell))?;
    
    m.add_wrapped(wrap_pyfunction!(get_all_runs))?;
    m.add_wrapped(wrap_pyfunction!(get_all_messages))?;
    m.add_wrapped(wrap_pyfunction!(get_last_trace))?;

    Ok(())
}

