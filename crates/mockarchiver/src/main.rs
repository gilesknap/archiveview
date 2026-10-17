//! `archiveview-mockarchiver [--port N] [--bind ADDR]`: serve synthetic
//! archived PVs on one port until killed.

use clap::Parser;

#[derive(Parser)]
#[command(name = "archiveview-mockarchiver", version, max_term_width = 100)]
/// A mock EPICS Archiver Appliance serving synthetic PV history.
struct Cli {
    /// The port to serve on (retrieval and mgmt endpoints alike).
    #[arg(long, default_value_t = mockarchiver::DEFAULT_PORT)]
    port: u16,
    /// The address to bind.
    #[arg(long, default_value = "127.0.0.1")]
    bind: std::net::IpAddr,
}

fn main() {
    let cli = Cli::parse();
    match mockarchiver::MockArchiver::start((cli.bind, cli.port).into()) {
        Ok(server) => {
            eprintln!("archiveview-mockarchiver: serving {}", server.url());
            server.wait();
        }
        Err(e) => {
            eprintln!("archiveview-mockarchiver: {e}");
            std::process::exit(1);
        }
    }
}
