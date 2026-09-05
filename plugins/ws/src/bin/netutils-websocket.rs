// `netutils-websocket` 是保留的旧命令别名，行为与 `netutils-ws` 完全相同。

#[tokio::main]
async fn main() {
    netutils_plugin_ws::run_cli().await;
}
