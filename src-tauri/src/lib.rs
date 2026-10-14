/// App-level commands exposed to the webview.
///
/// Commands stay thin: they resolve shell context (product identity from the
/// Tauri bundle, DB path, credential-store secrets) and delegate to
/// `roundtable-core`, which is shared with the Android shell.
mod commands;
mod remote;
mod secrets;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .setup(|app| {
            if cfg!(debug_assertions) {
                app.handle().plugin(
                    tauri_plugin_log::Builder::default()
                        .level(log::LevelFilter::Info)
                        .build(),
                )?;
            }
            let db_path = app
                .path()
                .app_data_dir()
                .map(|dir| dir.join("roundtable.db"))?;
            // Open once at startup so a corrupt/unwritable location fails
            // loudly here instead of on the first command.
            let _store = roundtable_core::KbStore::open(&db_path)?;
            _store.migrate_chat()?;
            remote::init_pairing_slot();
            app.manage(commands::AppState {
                db_path,
                streams: Mutex::new(HashMap::new()),
                room_controls: Arc::new(Mutex::new(HashMap::new())),
                remote: Mutex::new(commands::RemoteServerState::default()),
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::app_info,
            commands::providers_list,
            commands::providers_add,
            commands::providers_update,
            commands::providers_remove,
            commands::provider_key_has,
            commands::providers_test,
            commands::provider_models,
            commands::ollama_status,
            commands::conversations_list,
            commands::conversations_search,
            commands::conversations_create,
            commands::conversations_rename,
            commands::conversations_delete,
            commands::conversation_get,
            commands::conversations_set_model,
            commands::messages_list,
            commands::chat_send,
            commands::chat_regenerate,
            commands::chat_edit_resend,
            commands::chat_cancel,
            commands::agents_search,
            commands::agents_get,
            commands::agents_create,
            commands::agents_update,
            commands::agents_delete,
            commands::agents_duplicate,
            commands::agents_export,
            commands::agents_import,
            commands::agent_templates,
            commands::agent_test,
            commands::conversations_set_agent,
            commands::run_info,
            commands::rooms_list,
            commands::rooms_create,
            commands::rooms_get,
            commands::rooms_update,
            commands::rooms_delete,
            commands::rooms_set_members,
            commands::room_messages_list,
            commands::rooms_start,
            commands::rooms_pause,
            commands::rooms_resume,
            commands::rooms_step,
            commands::rooms_stop,
            commands::rooms_run_state,
            commands::agents_set_protocol,
            commands::mcp_servers,
            commands::mcp_add_server,
            commands::mcp_remove_server,
            commands::mcp_set_enabled,
            commands::mcp_tools,
            commands::agents_tools,
            commands::agents_set_tools,
            commands::agents_permissions,
            commands::agents_set_permissions,
            commands::workspace_dir,
            commands::workspace_pick_dir,
            commands::workspace_clear_dir,
            commands::tools_set_search_key,
            commands::tools_search_key_has,
            commands::audit_list,
            commands::rooms_approve,
            commands::remote_status,
            commands::remote_start,
            commands::remote_stop,
            commands::remote_pairing_code,
            commands::remote_tokens,
            commands::remote_revoke,
            commands::remote_client_status,
            commands::remote_connect,
            commands::remote_disconnect,
            commands::settings_get,
            commands::settings_set,
            commands::data_export,
            commands::data_import,
            commands::data_dir,
            commands::open_data_dir,
        ])
        .run(tauri::generate_context!())
        .expect("error while building Roundtable application");
}
