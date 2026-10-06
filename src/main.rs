mod clinic_relay;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    clinic_relay::run().await
}
