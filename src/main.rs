#[macro_use]
extern crate log;
extern crate clap;
extern crate libc;
extern crate syslog;

extern crate hex;
extern crate linux_embedded_hal as hal;
extern crate mfrc522;
extern crate rodio;

use clap::{App, Arg, ArgMatches};
use core::convert::TryFrom;
use hal::spidev::SpidevOptions;
use hal::sysfs_gpio::Direction;
use hal::{Pin, Spidev};
use log::LevelFilter;
use mfrc522::Mfrc522;
use nix::sys::signal::{signal, SigHandler, Signal};
use std::collections::HashMap;
use std::env;
use std::ffi::OsStr;
use std::fs::OpenOptions;
use std::io::{BufRead, BufReader, Error, ErrorKind, Result};
use std::path::PathBuf;
use std::process;
use std::thread;
use std::time::Duration;

extern "C" fn handle_signals(signal: libc::c_int) {
    let signal = Signal::try_from(signal).unwrap();
    info!("Signal {} received. Quitting.", signal.as_str());
    process::exit(1);
}

fn setup_rfid_reader() -> std::result::Result<Mfrc522<Spidev, Pin>, hal::sysfs_gpio::Error> {
    let mut spi = Spidev::open("/dev/spidev0.0")?;
    let options = SpidevOptions::new()
        .max_speed_hz(1_000_000)
        .mode(hal::spidev::SPI_MODE_0)
        .build();
    spi.configure(&options)?;

    let pin = Pin::new(25);
    pin.export()?;
    while !pin.is_exported() {}
    pin.set_direction(Direction::Out)?;
    pin.set_value(1)?;

    let mut mfrc522 = Mfrc522::new(spi, pin)?;
    let vers = mfrc522.version()?;

    info!("VERSION: 0x{:x}", vers);
    if vers == 0x91 || vers == 0x92 {
        Ok(mfrc522)
    } else {
        Err(hal::sysfs_gpio::Error::Unexpected(
            "Can't initialize rfid reader".to_string(),
        ))
    }
}

fn setup_signals() {
    let handler = SigHandler::Handler(handle_signals);
    unsafe {
        signal(Signal::SIGINT, handler).unwrap();
        signal(Signal::SIGHUP, handler).unwrap();
        signal(Signal::SIGQUIT, handler).unwrap();
    }
}

fn files_directory(arg_dir: Option<&str>) -> Result<String> {
    let current_dir: String = env::current_dir()?.to_str().unwrap().to_string();
    let dir = arg_dir.map(|dir| dir.to_string()).unwrap_or(current_dir);
    Ok(dir)
}

fn read_maps(mapping_file: &OsStr) -> Result<HashMap<String, String>> {
    info!("Reading mapping file");
    let mut maps = HashMap::new();
    let mapping_file = OpenOptions::new()
        .read(true)
        .write(false)
        .open(mapping_file)?;
    let mapping_buf = BufReader::new(mapping_file);
    for (line_idx, line) in mapping_buf.lines().enumerate() {
        let line = line?;
        let line = line.trim();
        if line.is_empty() || line.find('#') == Some(0) {
            continue;
        }
        let (key, file) = match line.find(' ') {
            Some(indx) => {
                let (k, v) = line.split_at(indx);
                (k.trim(), v.trim())
            }
            None => {
                return Err(Error::new(
                    ErrorKind::InvalidData,
                    format!("Line {}: '{}' format wrong", line_idx, line),
                ));
            }
        };
        debug!("map: {} - {}", key, file);
        maps.insert(key.to_string(), file.to_string());
    }
    Ok(maps)
}

struct FileMapper {
    files_dir: PathBuf,
    mapping: HashMap<String, String>,
}

impl FileMapper {
    pub fn new(arg_dir: Option<&str>, mapping_file: &OsStr) -> Result<Self> {
        let files_dir = files_directory(arg_dir)?.into();
        let mapping = read_maps(mapping_file)?;
        Ok(FileMapper { files_dir, mapping })
    }

    pub fn get_file(&self, hex_code: &str) -> Option<PathBuf> {
        let file_name = self.mapping.get(hex_code);
        file_name.map(|file_name| self.files_dir.join(file_name))
    }
}

fn main_loop(
    device: rodio::Device,
    mut mfrc522: Mfrc522<Spidev, Pin>,
    file_mapper: FileMapper,
) -> Result<()> {
    let mut playing: Option<String> = None;
    let mut current_sink: Option<rodio::Sink> = None;
    loop {
        if let Ok(uid) = mfrc522.reqa().and_then(|atqa| mfrc522.select(&atqa)) {
            let encoded_id = hex::encode(uid.bytes());
            if Some(&encoded_id) == playing.as_ref() {
                continue;
            }
            let fname = file_mapper.get_file(&encoded_id);
            let fname = match fname {
                Some(file_name) => file_name,
                None => {
                    error!("Card with id {} is not mapped", encoded_id);
                    continue;
                }
            };
            match OpenOptions::new().read(true).write(false).open(&fname) {
                Ok(opened_file) => {
                    if let Ok(new_sink) = rodio::play_once(&device, BufReader::new(opened_file)) {
                        let old_sink = current_sink.replace(new_sink);
                        if let Some(sink) = old_sink {
                            sink.stop();
                        }
                        info!("Playing {}", fname.display());
                    }
                }
                Err(error) => {
                    error!("Error opening {}: {}", fname.display(), error);
                }
            }
            playing.replace(encoded_id);
        } else {
            if let Some(sink) = current_sink.as_ref() {
                if sink.empty() {
                    current_sink.take();
                    playing.take();
                }
            }
        }
        thread::sleep(Duration::from_millis(500));
    }
}

fn run(matches: ArgMatches) -> Result<()> {
    debug!("Setup rfid reader");
    let mfrc522 =
        setup_rfid_reader().map_err(|err| Error::new(ErrorKind::Other, err.to_string()))?;
    debug!("Setup audio");
    let audio_device = rodio::default_output_device()
        .ok_or_else(|| Error::new(ErrorKind::NotFound, "Audio could not be opened"))?;
    debug!("Setup mapping structures");
    let mapper = FileMapper::new(
        matches.value_of("directory"),
        matches.value_of_os("mapping_file").unwrap(),
    )?;
    info!("Rfid player started");
    main_loop(audio_device, mfrc522, mapper)
}

fn main() {
    debug!("Start");
    setup_signals();
    syslog::init_unix(syslog::Facility::LOG_SYSLOG, LevelFilter::Debug).unwrap();
    let matches = App::new("rfid-audio")
        .about("Play mp3 files based on rfid sensor")
        .arg(
            Arg::with_name("directory")
                .short("d")
                .value_name("DIRECTORY")
                .help("Directory where mp3 files are present")
                .takes_value(true),
        )
        .arg(
            Arg::with_name("mapping_file")
                .short("m")
                .value_name("FILE")
                .help("Mapping file")
                .takes_value(true)
                .required(true),
        )
        .get_matches();
    debug!("Init done");
    match run(matches) {
        Err(e) => {
            error!("We caught an error: {}", e);
        }
        _ => {
            error!("We shouldn't have reached here");
        }
    }
}
