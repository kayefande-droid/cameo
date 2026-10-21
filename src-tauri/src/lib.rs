mod commands;
use commands::ai;
use commands::browser;
use commands::gateway;
use commands::mcp;
use commands::operator;
use commands::devtools;
use commands::power;
use commands::window::{dock_reposition, set_window_mode};
use commands::store::{
    db_ping, open_db, pref_get, pref_set, proxy_delete, proxy_list, proxy_save, proxy_test,
    agent_run_cancel, agent_run_create, agent_run_finish, agent_run_list, agent_run_recover_expired, agent_run_start,
    memory_add, memory_delete, memory_list, memory_update,
    task_apply_proposal, task_delete, task_event_add, task_events, task_file_proposals, task_list,
    task_project_context, task_propose_files, task_reject_proposal, task_upsert, AppState,
};

pub fn run() {
    // Store app data in %LOCALAPPDATA%\CAMEO so it survives reinstalls.
    let app_data = std::env::var("LOCALAPPDATA")
        .map(|d| std::path::PathBuf::from(d).join("CAMEO"))
        .map_err(|_| ())
        .unwrap_or_else(|_| std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from(".")));
    let db_path = app_data.join("cameo.db");
    let conn = open_db(&db_path);
    conn.execute_batch(gateway::GATEWAY_SCHEMA_SQL)
        .expect("failed to initialize gateway schema");
    conn.execute_batch(mcp::MCP_SCHEMA_SQL)
        .expect("failed to initialize MCP server schema");
    let state = AppState::new(conn);

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .manage(state)
        .manage(std::sync::Arc::new(gateway::GatewayState::default()))
        .invoke_handler(tauri::generate_handler![
            ai::save_provider_key,
            ai::delete_provider_key,
            ai::has_provider_key,
            ai::list_models,
            ai::test_connection,
            ai::chat,
            ai::chat_cancel,
            ai::chat_tool_decision,
            power::power_action,
            power::power_status,
            operator::operator_status,
            operator::operator_configure,
            operator::operator_set_root,
            operator::operator_set_full_access,
            operator::exec_run,
            operator::net_fetch,
            operator::net_search,
            operator::search_set_key,
            operator::search_has_key,
            operator::search_delete_key,
            operator::device_launch,
            devtools::detect_dev_tools,
            devtools::inspect_project,
            devtools::run_project_command,
            browser::open_browser_window,
            set_window_mode,
            dock_reposition,
            pref_get,
            pref_set,
            task_upsert,
            task_list,
            task_events,
            task_project_context,
            task_event_add,
            agent_run_create,
            agent_run_list,
            agent_run_start,
            agent_run_finish,
            agent_run_cancel,
            agent_run_recover_expired,
            task_delete,
            task_propose_files,
            task_file_proposals,
            task_reject_proposal,
            task_apply_proposal,
            proxy_list,
            proxy_save,
            proxy_delete,
            proxy_test,
            memory_add,
            memory_list,
            memory_update,
            memory_delete,
            gateway::gw_stop_control,
            gateway::gw_stop_status,
            gateway::gw_registry,
            gateway::gw_status,
            gateway::gw_list_all,
            gateway::gw_get_permissions,
            gateway::gw_set_permissions,
            gateway::gw_set_secret,
            gateway::gw_has_secret,
            gateway::gw_clear_secrets,
            gateway::gw_oauth_connect,
            gateway::gw_oauth_refresh,
            gateway::gw_test_connection,
            gateway::gw_connect_api_key,
            gateway::gw_connector_call,
            gateway::gw_disconnect,
            gateway::gw_audit_list,
            mcp::mcp_add,
            mcp::mcp_list,
            mcp::mcp_update,
            mcp::mcp_remove,
            mcp::mcp_set_enabled,
            mcp::mcp_validate,
            mcp::mcp_inspect_tools,
            mcp::mcp_call_tool,
            db_ping
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
