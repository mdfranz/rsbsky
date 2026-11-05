#[tokio::main]
async fn main() {
    std::process::exit(rsbsky::cli::real_main().await);
}
