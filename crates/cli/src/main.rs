use clap::Parser;

#[derive(Parser, Debug)]
#[command(name = "mpr")]
#[command(about = "MemPalace - AI memory system", long_about = None)]
enum Cli {
    Init {
        #[arg(help = "Palace directory")]
        directory: Option<String>,
    },
    Mine {
        #[arg(help = "Directory to mine")]
        directory: String,
        #[arg(long, default_value = "projects")]
        mode: String,
        #[arg(long)]
        wing: Option<String>,
        #[arg(long)]
        extract: Option<String>,
        #[arg(long)]
        auto: bool,
    },
    Search {
        #[arg(help = "Search query")]
        query: String,
        #[arg(long)]
        wing: Option<String>,
        #[arg(long)]
        room: Option<String>,
    },
    Status,
    WakeUp {
        #[arg(long)]
        wing: Option<String>,
    },
    Split {
        #[arg(help = "Directory to split")]
        directory: String,
        #[arg(long, default_value = "false")]
        dry_run: bool,
        #[arg(long)]
        min_sessions: Option<usize>,
    },
    Doctor,
    Mcp,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    run().await
}

pub async fn run() -> anyhow::Result<()> {
    let cli = Cli::parse();

    match cli {
        Cli::Init { directory: _ } => {
            eprintln!("init command - not yet implemented");
        }
        Cli::Mine {
            directory: _,
            mode: _,
            wing: _,
            extract: _,
            auto: _,
        } => {
            eprintln!("mine command - not yet implemented");
        }
        Cli::Search {
            query: _,
            wing: _,
            room: _,
        } => {
            eprintln!("search command - not yet implemented");
        }
        Cli::Status => {
            eprintln!("status command - not yet implemented");
        }
        Cli::WakeUp { wing: _ } => {
            eprintln!("wake-up command - not yet implemented");
        }
        Cli::Split {
            directory: _,
            dry_run: _,
            min_sessions: _,
        } => {
            eprintln!("split command - not yet implemented");
        }
        Cli::Doctor => {
            eprintln!("doctor command - not yet implemented");
        }
        Cli::Mcp => {
            eprintln!("mcp command - not yet implemented");
        }
    }

    Ok(())
}
