use byteorder::{BigEndian, ReadBytesExt};
use chrono::{DateTime, Duration as ChronoDuration, FixedOffset, Local, TimeZone, Timelike, Utc};
use std::{
    collections::HashMap,
    fmt::{self, Debug, Display},
    net::{ToSocketAddrs, UdpSocket},
    time::Duration,
};
use thiserror::Error;

#[derive(Clone, Copy)]
pub struct Clock {
    time: DateTime<FixedOffset>,
}

impl Clock {
    pub fn new(dt: DateTime<Local>) -> Self {
        let dt = DateTime::<FixedOffset>::from_utc(dt.naive_utc(), *dt.offset());
        Self { time: dt }
    }

    pub fn now_with_offset(offset: f64) -> Self {
        let adjusted_dt = Local::now() + ChronoDuration::milliseconds(offset as i64);
        Self::new(adjusted_dt)
    }

    pub fn now_synced() -> Result<Self, LunartickError> {
        let ntp_client = NTPClient::new();
        let adjust_ms = ntp_client.test()?.get_time_millis();
        Ok(Self::now_with_offset(adjust_ms))
    }

    pub fn now() -> Self {
        let now = Local::now();
        Self::new(now)
    }

    pub fn from_rfc2822(dt: String) -> Result<Self, LunartickError> {
        let dt = DateTime::parse_from_rfc2822(&dt)
            .map_err(|_| LunartickError::ParseDateTimeError(DateTimeFormat::RFC2822))?;
        Ok(Self { time: dt })
    }

    pub fn from_rfc3339(dt: String) -> Result<Self, LunartickError> {
        let dt = DateTime::parse_from_rfc3339(&dt)
            .map_err(|_| LunartickError::ParseDateTimeError(DateTimeFormat::RFC3339))?;
        Ok(Self { time: dt })
    }

    pub fn to_string(&self) -> String {
        format!("{} {}", self.time.naive_local(), self.time.offset())
    }

    pub fn get_timestamp(&self) -> i64 {
        self.time.timestamp()
    }

    pub fn get_rfc2822(&self) -> String {
        self.time.to_rfc2822()
    }

    pub fn get_rfc3339(&self) -> String {
        self.time.to_rfc3339()
    }

    #[cfg(windows)]
    pub fn set(&self) -> Result<(), LunartickError> {
        use chrono::Datelike;
        use std::mem::zeroed;
        use windows::Win32::{Foundation::SYSTEMTIME, System::SystemInformation::SetSystemTime};

        let t = self.time;
        let mut systime: SYSTEMTIME = unsafe { zeroed() };
        let dow = t.weekday().num_days_from_sunday();
        let mut ns = t.nanosecond();
        let is_leap_second = ns > 1_000_000_000;
        if is_leap_second {
            ns -= 1_000_000_000;
        }
        systime.wYear = t.year() as u16;
        systime.wMonth = t.month() as u16;
        systime.wDayOfWeek = dow as u16;
        systime.wDay = t.day() as u16;
        systime.wHour = t.hour() as u16;
        systime.wMinute = t.minute() as u16;
        systime.wSecond = t.second() as u16;
        systime.wMilliseconds = (ns / 1_000_000) as u16;
        let systime_ptr = &systime as *const SYSTEMTIME;
        unsafe {
            SetSystemTime(systime_ptr);
        }
        catch_os_error()
    }

    #[cfg(not(windows))]
    pub fn set(&self) -> Result<(), LunartickError> {
        use libc::{settimeofday, suseconds_t, time_t, timeval, timezone};
        use std::mem::zeroed;

        let t = self.time;
        let mut u: timeval = unsafe { zeroed() };
        u.tv_sec = t.timestamp() as time_t;
        u.tv_usec = t.timestamp_subsec_micros() as suseconds_t;
        unsafe {
            let mock_tz: *const timezone = std::ptr::null();
            settimeofday(&u as *const timeval, mock_tz);
        }
        catch_os_error()
    }
}

fn catch_os_error() -> Result<(), LunartickError> {
    let maybe_error = std::io::Error::last_os_error();
    let os_error_code = &maybe_error.raw_os_error();
    match os_error_code {
        Some(0) => Ok(()),
        Some(_) => Err(LunartickError::SetError(maybe_error.to_string())),
        None => Ok(()),
    }
}

impl From<DateTime<Local>> for Clock {
    fn from(d: DateTime<Local>) -> Self {
        Clock::new(d)
    }
}

impl Display for Clock {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_string())
    }
}

impl Debug for Clock {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_string())
    }
}

impl From<Clock> for DateTime<Local> {
    fn from(c: Clock) -> Self {
        c.time.with_timezone(&Local)
    }
}

#[derive(Error, Debug)]
pub enum LunartickError {
    #[error("error parsing {0:?}")]
    ParseDateTimeError(DateTimeFormat),

    #[error("{0}")]
    SetError(String),

    #[error(transparent)]
    IO(#[from] std::io::Error),

    #[error("error connecting to server")]
    ConnectionError,

    #[error("error parsing timestamp")]
    ParseTimestampError,
}

#[derive(Debug, Clone)]
pub enum DateTimeFormat {
    RFC2822,
    RFC3339,
}

const NTP_MESSAGE_LENGTH: usize = 48;
const NTP_TO_UNIX_SECONDS: i64 = 2_208_988_800;
const LOCAL_ADDR: &str = "0.0.0.0:12300";

#[derive(Debug, Default, Copy, Clone)]
struct NTPTimestamp {
    seconds: u32,
    fraction: u32,
}

struct NTPMessage {
    data: [u8; NTP_MESSAGE_LENGTH],
}

#[derive(Debug, Clone)]
struct NTPResult {
    t1: DateTime<Utc>,
    t2: DateTime<Utc>,
    t3: DateTime<Utc>,
    t4: DateTime<Utc>,
}

impl NTPResult {
    fn delay(&self) -> i64 {
        let duration = (self.t4 - self.t1) - (self.t3 - self.t2);
        duration.num_milliseconds()
    }

    fn offset(&self) -> i64 {
        let delta = self.delay();
        delta.abs() / 2
    }
}

impl From<NTPTimestamp> for DateTime<Utc> {
    fn from(ntp: NTPTimestamp) -> Self {
        let secs = ntp.seconds as i64 - NTP_TO_UNIX_SECONDS;
        let mut nanos = ntp.fraction as f64;
        nanos *= 1e9;
        nanos /= 2_f64.powi(32);
        Utc.timestamp(secs, nanos as u32)
    }
}

impl From<DateTime<Utc>> for NTPTimestamp {
    fn from(utc: DateTime<Utc>) -> Self {
        let secs = utc.timestamp() + NTP_TO_UNIX_SECONDS;
        let mut fraction = utc.nanosecond() as f64;
        fraction *= 2_f64.powi(32);
        fraction /= 1e9;
        Self {
            seconds: secs as u32,
            fraction: fraction as u32,
        }
    }
}

impl NTPMessage {
    fn new() -> Self {
        NTPMessage {
            data: [0; NTP_MESSAGE_LENGTH],
        }
    }

    fn client() -> Self {
        const VERSION: u8 = 0b00_011_000;
        const MODE: u8 = 0b00_000_011;
        let mut msg = NTPMessage::new();
        msg.data[0] |= VERSION;
        msg.data[0] |= MODE;
        msg
    }

    fn parse_timestamp(&self, i: usize) -> Result<NTPTimestamp, std::io::Error> {
        let mut reader = &self.data[i..i + 8];
        let seconds = reader.read_u32::<BigEndian>()?;
        let fraction = reader.read_u32::<BigEndian>()?;
        Ok(NTPTimestamp { seconds, fraction })
    }

    fn rx_time(&self) -> Result<NTPTimestamp, std::io::Error> {
        self.parse_timestamp(32)
    }

    fn tx_time(&self) -> Result<NTPTimestamp, std::io::Error> {
        self.parse_timestamp(40)
    }
}

fn weighted_mean(values: &[f64], weights: &[f64]) -> f64 {
    let (result, sum_of_weights) = values
        .iter()
        .zip(weights)
        .fold((0.0, 0.0), |(result, sum_of_weights), (v, w)| {
            (result + v * w, sum_of_weights + w)
        });
    result / sum_of_weights
}

fn ntp_roundtrip<A: ToSocketAddrs>(host: A) -> Result<NTPResult, LunartickError> {
    let timeout = Duration::from_secs(1);
    let request = NTPMessage::client();
    let mut response = NTPMessage::new();
    let message = request.data;
    let udp = UdpSocket::bind(LOCAL_ADDR)?;
    udp.connect(host)
        .map_err(|_| LunartickError::ConnectionError)?;
    let t1 = Utc::now();
    udp.send(&message)?;
    udp.set_read_timeout(Some(timeout))?;
    udp.recv_from(&mut response.data)?;
    let t4 = Utc::now();
    let t2: DateTime<Utc> = response
        .rx_time()
        .map_err(|_| LunartickError::ParseTimestampError)?
        .into();
    let t3: DateTime<Utc> = response
        .tx_time()
        .map_err(|_| LunartickError::ParseTimestampError)?
        .into();
    Ok(NTPResult { t1, t2, t3, t4 })
}

#[derive(Debug, Clone)]
pub struct TestResults {
    result: HashMap<String, Option<NTPResult>>,
}

impl TestResults {
    pub fn get_all_results(&self) -> HashMap<String, Option<i64>> {
        self.result
            .iter()
            .map(|(server, ntp_result)| {
                (server.to_owned(), ntp_result.as_ref().map(|r| r.offset()))
            })
            .collect()
    }

    pub fn get_time_millis(&self) -> f64 {
        let mut offsets = Vec::with_capacity(self.result.len());
        let mut offset_weights = Vec::with_capacity(self.result.len());
        self.result
            .iter()
            .filter_map(|r| r.1.as_ref())
            .filter_map(|time| {
                let offset = time.offset() as f64;
                let delay = time.delay() as f64;
                let weight = 1_000_000.0 / (delay * delay);
                if weight.is_finite() {
                    Some((offset, weight))
                } else {
                    None
                }
            })
            .for_each(|(offset, weight)| {
                offsets.push(offset);
                offset_weights.push(weight);
            });
        let avg_offset = weighted_mean(&offsets, &offset_weights);
        avg_offset
    }
}

#[derive(Debug, Clone)]
pub struct NTPClient {
    servers: Vec<String>,
}

impl Default for NTPClient {
    fn default() -> Self {
        let servers = vec![
            "time.nist.gov".to_owned(),
            "time.apple.com".to_owned(),
            "time.euro.apple.com".to_owned(),
            "time.google.com".to_owned(),
            "time2.google.com".to_owned(),
            // "time.windows.com".to_owned(),
        ];
        Self { servers }
    }
}

impl NTPClient {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn new_with_server(server: String) -> Self {
        Self {
            servers: vec![server],
        }
    }

    pub fn new_with_multiple_servers(servers: Vec<String>) -> Self {
        Self { servers }
    }

    pub fn get_servers(&self) -> Vec<String> {
        self.servers.clone()
    }

    pub fn test(&self) -> Result<TestResults, LunartickError> {
        const NTP_PORT: u16 = 123;
        let mut times = Vec::with_capacity(self.servers.len());
        for server in &self.servers {
            let destination = format!("{}:{}", server, NTP_PORT);
            let calc = ntp_roundtrip(destination);
            match calc {
                Err(e)
                    if matches!(
                        e,
                        LunartickError::ConnectionError | LunartickError::ParseTimestampError
                    ) =>
                {
                    return Err(e);
                }
                _ => times.push(calc.ok()),
            }
        }
        let result = times
            .into_iter()
            .zip(&self.servers)
            .map(|(score, server)| (server.to_owned(), score))
            .collect();
        Ok(TestResults { result })
    }
}
