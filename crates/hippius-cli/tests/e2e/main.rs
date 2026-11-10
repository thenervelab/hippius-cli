//! End-to-end tests that run the `hipc` binary against a local scripted server. No test
//! talks to api.hippius.com. One binary holds every suite, so the shared helpers in
//! `common` are all used. `manual_test` runs `scripts/manual_test.sh` with `--offline`,
//! against a dead origin.

mod account;
mod api;
mod app_engine;
mod app_engine_create;
mod auth;
mod backups;
mod billing;
mod billing_reads;
mod billing_writes;
mod cdn;
mod common;
mod compute;
mod compute_reads;
mod config;
mod contract;
mod databases;
#[path = "../shared/dead_port.rs"]
mod dead_port;
mod dns_setup;
mod drive;
mod examples;
mod expose;
mod file_inputs;
mod gamma;
mod halpha;
mod hub;
mod hub_keys;
mod hub_models;
mod hub_namespace;
mod hub_registry;
mod invites;
mod invoices;
mod kubernetes;
mod kubernetes_keyed;
mod login_prompt;
mod manual_test;
mod network;
mod network_create;
mod next_steps;
mod notifications;
mod pagination;
mod power_policy;
mod presign;
mod project_writes;
mod projects;
mod prompt;
mod public_ip;
mod referrals;
mod runners;
mod s3;
mod s3_objects;
mod secret_file;
mod server_words;
mod sources;
mod status;
mod support;
mod team;
mod top_up;
mod transport;
mod voice;
mod waf;
