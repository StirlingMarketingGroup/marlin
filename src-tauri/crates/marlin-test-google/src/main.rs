//! Serves the fake Google for the real-app tests: prints its address on the first line of stdout, and stops when stdin closes. So it stops when the test is done with it, and also when the test process dies.

use tokio::io::AsyncReadExt;

#[tokio::main]
async fn main() {
    let (server, _state) = marlin_test_google::start().await;
    println!("{}", server.uri());
    let _ = tokio::io::stdin().read_to_end(&mut Vec::new()).await;
}
