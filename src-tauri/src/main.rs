// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

pub mod commands;
pub mod structs;
pub mod enums;
pub mod database;

use std::fs;
use std::time::Duration;
use sqlx::{SqlitePool, sqlite::SqliteConnectOptions};
use commands::fetcher::{sync, sync_all};
use commands::splashscreen::{close_splashscreen, open_main_window};
use structs::single_instance_payload::SingleInstancePayload;
use tauri::{generate_handler, generate_context, Manager, Builder, SystemTray, SystemTrayEvent, SystemTrayMenu, CustomMenuItem, AppHandle, Wry};
use tauri::async_runtime::block_on;
use tauri_plugin_autostart::MacosLauncher;
use tauri_plugin_window_state::StateFlags;
use crate::database::load_migrations;

fn show_main_window(app: &AppHandle<Wry>) {
    let window = app.get_window("main").unwrap();
    window.show().unwrap();
}

fn fully_close_app(app: &AppHandle<Wry>) {
    let window = app.get_window("main").unwrap();
    window.close().unwrap();
}

fn main() {
    let quit = CustomMenuItem::new("quit".to_string(), "Quit");
    let show = CustomMenuItem::new("show".to_string(), "Show Alduin");

    let tray_menu = SystemTrayMenu::new()
        .add_item(show)
        .add_item(quit);
    let system_tray = SystemTray::new()
        .with_menu(tray_menu);

    let mut flags = StateFlags::all();
    flags.remove(StateFlags::VISIBLE);

    Builder::default()
        .plugin(tauri_plugin_window_state::Builder::default()
            .with_state_flags(flags)
            .build())
        .plugin(tauri_plugin_sql::Builder::default().add_migrations("sqlite:alduin.db", load_migrations()).build())
        .plugin(tauri_plugin_store::Builder::default().build())
        .plugin(tauri_plugin_autostart::init(MacosLauncher::LaunchAgent, Some(vec!["--autostart"])))
        .plugin(tauri_plugin_single_instance::init(|app, argv, cwd| {
            app.emit_all("single-instance", SingleInstancePayload { args: argv, cwd }).unwrap();
        }))
        .invoke_handler(generate_handler![sync, sync_all, close_splashscreen, open_main_window])
        .system_tray(system_tray)
        .on_system_tray_event(|app, event| match event {
            SystemTrayEvent::DoubleClick {
                position: _,
                size: _,
                ..
            } => {
                show_main_window(app)
            }
            SystemTrayEvent::MenuItemClick { id, .. } => match id.as_str() {
                "quit" => {
                    fully_close_app(app)
                }
                "show" => {
                    show_main_window(app)
                }
                _ => {}
            },
            _ => {}
        })
        .setup(|app| {
             block_on(async move {
                 let handle = app.handle();
                 
                 eprintln!("=== ALDUIN DATABASE SETUP DEBUG ===");
                 eprintln!("Starting database initialization...");

                 // Use plugin's config directory to access same database file
                 let app_dir = if let Some(native_dir) = handle.path_resolver().app_config_dir() {
                     eprintln!("✅ Using Tauri app config directory: {:?}", native_dir);
                     native_dir
                 } else if let Ok(home) = std::env::var("HOME") {
                     let fallback_dir = std::path::PathBuf::from(&home).join(".config/io.stouder.alduin");
                     eprintln!("⚠️  Falling back to HOME/.config/io.stouder.alduin: {:?}", fallback_dir);
                     fallback_dir
                 } else {
                     let emergency_dir = std::path::PathBuf::from("./data");
                     eprintln!("🚨 Emergency fallback to ./data: {:?}", emergency_dir);
                     emergency_dir
                 };

                 // Debug: Log path information  
                 eprintln!("Plugin config directory: {:?}", app_dir);
                 eprintln!("Directory exists: {}", app_dir.exists());

                 // Connect to existing plugin database
                 let sqlite_path = app_dir.join("alduin.db");
                 
                 eprintln!("Plugin database path: {:?}", sqlite_path);
                 eprintln!("Database file exists: {}", sqlite_path.exists());
                 
                 if sqlite_path.exists() {
                     if let Ok(metadata) = fs::metadata(&sqlite_path) {
                         eprintln!("Database file size: {} bytes", metadata.len());
                         eprintln!("Database file readonly: {}", metadata.permissions().readonly());
                     }
                 } else {
                     eprintln!("⚠️  Plugin database not yet created, will retry connection...");
                 }

                 // Create connection options without create_if_missing (plugin handles creation)
                 let connect_options = SqliteConnectOptions::new()
                     .filename(&sqlite_path);

                 // Wait for plugin initialization and database creation
                 eprintln!("Waiting for plugin initialization and database creation...");
                 tokio::time::sleep(Duration::from_millis(1000)).await;
                 
                 // Verify plugin database exists before connecting
                 if !sqlite_path.exists() {
                     eprintln!("⚠️  Plugin database still not found, waiting longer...");
                     tokio::time::sleep(Duration::from_millis(2000)).await;
                     
                     if !sqlite_path.exists() {
                         eprintln!("❌ Plugin database not found after extended wait");
                         eprintln!("❌ Expected location: {:?}", sqlite_path);
                         panic!("Plugin database creation failed or path mismatch");
                     }
                 }

                 // Connect to plugin database
                 eprintln!("Connecting to plugin database...");
                 let mut connection_attempts = 0;
                 let max_attempts = 3;
                 
                 let db = loop {
                     connection_attempts += 1;
                     eprintln!("Plugin database connection attempt {}/{}", connection_attempts, max_attempts);
                     
                     match SqlitePool::connect_with(connect_options.clone()).await {
                         Ok(pool) => {
                             eprintln!("✅ Plugin database connection successful on attempt {}", connection_attempts);
                             break pool;
                         }
                         Err(e) => {
                             eprintln!("❌ SQLite connection failed on attempt {}: {}", connection_attempts, e);
                             eprintln!("Connection options - filename: {:?}", sqlite_path);
                             eprintln!("Database path: {:?}", sqlite_path);
                             eprintln!("Directory exists: {}", app_dir.exists());
                             eprintln!("Database file exists: {}", sqlite_path.exists());
                             
                             // Additional debugging for SQLite-specific errors
                             match e {
                                 sqlx::Error::Database(ref db_err) => {
                                     eprintln!("Database error code: {:?}", db_err.code());
                                     eprintln!("Database error message: {}", db_err.message());
                                 }
                                 _ => {
                                     eprintln!("Non-database error: {:?}", e);
                                 }
                             }
                             
                             if connection_attempts >= max_attempts {
                                 eprintln!("❌ All connection attempts failed. Final error: {}", e);
                                 panic!("Failed to connect to SQLite after {} attempts: {}", max_attempts, e);
                             }
                             
                             // Wait before retrying with exponential backoff
                             let delay = Duration::from_millis(100 * connection_attempts as u64);
                             eprintln!("Retrying in {}ms...", delay.as_millis());
                             tokio::time::sleep(delay).await;
                         }
                     }
                 };

                 eprintln!("✅ Plugin database connection established!");
                 eprintln!("Registering unified database with app state...");
                 app.manage(db);
                 eprintln!("✅ Unified database registered with app state");
                 eprintln!("=== UNIFIED DATABASE SETUP COMPLETE ===");

                 Ok(())
            })
        })
        .run(generate_context!())
        .expect("error while running tauri application");
}

