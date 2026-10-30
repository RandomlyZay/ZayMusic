use std::net::SocketAddr;

#[tokio::main]
async fn main() {
    let config = zay_gateway::Config::from_env().unwrap_or_else(|message| {
        eprintln!("zay-gateway: {message}");
        std::process::exit(2);
    });
    let host = std::env::var("HOST").unwrap_or_else(|_| "127.0.0.1".into());
    let port = std::env::var("PORT").unwrap_or_else(|_| "3000".into());
    let address: SocketAddr = format!("{host}:{port}").parse().unwrap_or_else(|_| {
        eprintln!("zay-gateway: HOST/PORT do not form a socket address");
        std::process::exit(2);
    });
    let listener = tokio::net::TcpListener::bind(address)
        .await
        .unwrap_or_else(|failure| {
            eprintln!("zay-gateway: cannot listen on {address}: {failure}");
            std::process::exit(1);
        });
    eprintln!("zay-gateway {} listening on {address}", config.build_sha);
    axum::serve(listener, zay_gateway::router(config))
        .with_graceful_shutdown(shutdown())
        .await
        .expect("gateway server failed");
}

/// Docker stops the container with SIGTERM; terminals send SIGINT.
async fn shutdown() {
    let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .expect("SIGTERM handler");
    tokio::select! {
        _ = term.recv() => {}
        _ = tokio::signal::ctrl_c() => {}
    }
}
