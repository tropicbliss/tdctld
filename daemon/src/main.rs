use anyhow::{Context, Result};
use clap::{clap_derive::ArgEnum, Parser, Subcommand};
use lunartick::{Clock, LunartickError, NTPClient};
use std::time::Duration;
use tracing::{error, info, warn};

fn main() -> Result<()> {
    if std::env::var_os("RUST_LOG").is_none() {
        std::env::set_var("RUST_LOG", "info");
    }
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .without_time()
        .init();
    let args = Args::parse();
    match args.command {
        Commands::Get { std } => get(std),
        Commands::Set { std, datetime } => set(std, datetime)?,
        Commands::Sync { servers } => sync(servers)?,
        Commands::Daemon { servers, timeout } => daemon(servers, timeout)?,
    }
    Ok(())
}

#[derive(Parser)]
#[clap(version, about)]
struct Args {
    #[clap(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Get current time info
    Get {
        /// Date/time format
        #[clap(arg_enum, default_value = "debug")]
        std: GetDTFormats,
    },

    /// Set system time
    Set {
        /// Date/time format
        #[clap(arg_enum, short, long)]
        std: SetDTFormats,

        /// Date/time to set to
        datetime: String,
    },

    /// Synchronize system clock with NTP servers
    Sync {
        /// NTP servers to synchronize against
        #[clap(short, long)]
        servers: Option<Vec<String>>,
    },

    /// Run tdctld as a background process to synchronize the system clock in set intervals (only available on Linux)
    Daemon {
        /// NTP servers to synchronize against
        #[clap(short, long)]
        servers: Option<Vec<String>>,

        /// Duration between synchronizations (in seconds)
        #[clap(default_value = "1800")]
        timeout: u64,
    },
}

#[derive(ArgEnum, Clone)]
enum GetDTFormats {
    Debug,
    Timestamp,
    RFC2822,
    RFC3339,
}

#[derive(ArgEnum, Clone, Debug)]
enum SetDTFormats {
    RFC2822,
    RFC3339,
}

impl From<SetDTFormats> for GetDTFormats {
    fn from(value: SetDTFormats) -> Self {
        match value {
            SetDTFormats::RFC2822 => GetDTFormats::RFC2822,
            SetDTFormats::RFC3339 => GetDTFormats::RFC3339,
        }
    }
}

fn get(std: GetDTFormats) {
    let now = Clock::now();
    match std {
        GetDTFormats::Debug => info!("{now:?}"),
        GetDTFormats::Timestamp => info!("{}", now.get_timestamp()),
        GetDTFormats::RFC2822 => info!("{}", now.get_rfc2822()),
        GetDTFormats::RFC3339 => info!("{}", now.get_rfc3339()),
    }
}

fn set(std: SetDTFormats, datetime: String) -> Result<()> {
    let parser = match std {
        SetDTFormats::RFC2822 => Clock::from_rfc2822,
        SetDTFormats::RFC3339 => Clock::from_rfc3339,
    };
    let dt = parser(datetime.clone())
        .context(format!("Unable to parse {datetime} according to {std:?}"))?;
    let res = dt.set();
    match res {
        Err(LunartickError::SetError(e)) => error!(e),
        Err(e) => return Err(e.into()),
        _ => (),
    }
    get(std.into());
    Ok(())
}

fn sync(servers: Option<Vec<String>>) -> Result<()> {
    let ntp_client = if let Some(servers) = servers {
        NTPClient::new_with_multiple_servers(servers)
    } else {
        NTPClient::new()
    };
    let results = ntp_client.test()?;
    let raw_timings = results.get_all_results();
    raw_timings.into_iter().for_each(|(server, timing)| {
        if let Some(time) = timing {
            info!("{server} => {time}ms away from local system time");
        } else {
            warn!("{server} => ? [response took too long]");
        }
    });
    let offset = results.get_time_millis();
    let adjusted_dt = Clock::now_with_offset(offset);
    let res = adjusted_dt.set();
    match res {
        Err(LunartickError::SetError(e)) => error!(e),
        Err(e) => return Err(e.into()),
        _ => (),
    }
    get(GetDTFormats::Debug);
    Ok(())
}

#[allow(clippy::too_many_lines)]
fn daemon(servers: Option<Vec<String>>, timeout: u64) -> Result<()> {
    info!("starting daemon service");
    loop {
        sync(servers.clone())?;
        std::thread::sleep(Duration::from_secs(timeout));
    }
}
