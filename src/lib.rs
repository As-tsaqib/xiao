pub mod agent;
pub mod app;
pub mod auth;
pub mod command;
pub mod config;
pub mod event;
pub mod ipc;
pub mod providers;
pub mod security;
pub mod session;
pub mod standalone;
pub mod storage;
pub mod telegram;
pub mod tools;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub mod presentation;
