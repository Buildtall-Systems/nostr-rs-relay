fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Existing: authorization client only
    tonic_build::configure()
        .build_server(false)
        .protoc_arg("--experimental_allow_proto3_optional")
        .compile(&["proto/nauthz.proto"], &["proto"])?;
    // Relay service: both client and server
    tonic_build::configure()
        .build_server(true)
        .build_client(true)
        .compile(&["proto/relay.proto"], &["proto"])?;
    Ok(())
}
