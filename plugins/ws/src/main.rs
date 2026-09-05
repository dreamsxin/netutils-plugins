// `netutils-ws` 入口：实现在 lib 中，见 netutils_plugin_ws::run_cli。

#[tokio::main]
async fn main() {
    netutils_plugin_ws::run_cli().await;
}
