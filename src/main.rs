#[macro_use]
extern crate log;
extern crate clap;
extern crate libc;
extern crate simple_logger;

extern crate hex;
extern crate linux_embedded_hal as hal;
extern crate mfrc522;
extern crate rodio;

use std::io::BufReader;

use clap::{App, Arg, ArgMatches};
use core::convert::TryFrom;
use hal::spidev::SpidevOptions;
use hal::sysfs_gpio::Direction;
use hal::{Pin, Spidev};
use mfrc522::Mfrc522;
use nix::sys::signal::{signal, SigHandler, Signal};
use std::collections::HashMap;
use std::env;
use std::ffi::OsStr;
use std::fs::File;
use std::io::{self, BufRead, BufReader, Result};
use std::path::PathBuf;
use std::process;
use std::thread;
use std::time::Duration;

extern "C" fn handle_signals(signal: libc::c_int) {
    let signal = Signal::try_from(signal).unwrap();
    info!("Signal {} received. Quitting.", signal.as_str());
    process::exit(1);
}

fn setup_rfid_reader() -> Result<Mfrc522<Spidev, Pin>> {
    let mut spi = Spidev::open("/dev/spidev0.0").unwrap();
    let options = SpidevOptions::new()
        .max_speed_hz(1_000_000)
        .mode(hal::spidev::SPI_MODE_0)
        .build();
    spi.configure(&options).unwrap();

    let pin = Pin::new(25);
    pin.export().unwrap();
    while !pin.is_exported() {}
    pin.set_direction(Direction::Out).unwrap();
    pin.set_value(1).unwrap();

    let mut mfrc522 = Mfrc522::new(spi, pin).unwrap();
    let vers = mfrc522.version().unwrap();

    info!("VERSION: 0x{:x}", vers);
    if vers == 0x91 || vers == 0x92 {
        Ok(mfrc522)
    } else {
        Err(Error::new(ErrorKind::NotFound, "Couldn't find RFID reader")
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

fn files_dir(arg_dir: Option<&str>) -> Result<String> {
    let current_dir: String = env::current_dir()?.to_str().unwrap().to_string();
    let dir = arg_dir.map(|dir| dir.to_string()).unwrap_or(current_dir);
    Ok(dir)
}

fn read_maps(mapping_file: &OsStr) -> Result<HashMap<String, String>> {
    let mut maps = HashMap::new();
    let mapping_file = File::open(mapping_file)?;
    let mapping_buf = BufReader::new(mapping_file);
    for line in mapping_buf.lines() {
        let line = line?;
        let fields: Vec<&str> = line.split(" ").collect();
        maps.insert(fields[0].to_string(), fields[fields.len() - 1].to_string());
    }
    Ok(maps)
}

struct FileMapper {
    files_dir: PathBuf,
    mapping: HashMap<String, String>,
}

impl FileMapper {
    fn new(files_dir: String, mapping: HashMap<String, String>) -> Self {
        let files_dir = files_dir.into();
        FileMapper { files_dir, mapping }
    }

    fn get_file(&self, hex_code: &str) -> Option<PathBuf> {
        let file_name = self.mapping.get(hex_code);
        file_name.map(|file_name| self.files_dir.join(file_name))
    }
}

fn main_loop(device: rodio::Device, mfrc522: Mfrc522<Spidev, Pin>, file_mapper: FileMapper) -> Result<()> {
    let mut playing: Option<String> = None;
    let mut current_sink: Option<rodio::Sink> = None;
    loop {
        if let Ok(uid) = mfrc522.reqa().and_then(|atqa| mfrc522.select(&atqa)) {
            let encoded_id = hex::encode(uid.bytes());
            if Some(&encoded_id) == playing.as_ref() {
                continue;
            }
            let fname = file_mapper.get_file(encoded_id);
            let fname = match fname {
                Ok(file_name) => file_name,
                None => {
                    error!("Card with id {} is not mapped", encoded_id);
                    continue;
                }
            }
            match File::open(&fname) {
                Ok(opened_file) => {
                    if let Ok(new_sink) = rodio::play_once(&device, BufReader::new(opened_file))
                    {
                        let old_sink = current_sink.replace(new_sink);
                        if let Some(sink) = old_sink {
                            sink.stop();
                        }
                        info!("Playing {}", fname);
                    }
                }
                Err(error) => {
                    error!("Error opening {}: {}", fname, error);
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
    let mfrc522 = setup_rfid_reader()?;
    let audio_device = rodio::default_output_device().ok_or_else(|| Error::new(ErrorKind::NotFound, "Audio could not be opened"))?;
    let dir = files_dir(matches.value_of("directory"))?;
    let map = read_maps(matches.value_of_os("mapping_file").unwrap())?;
    let mapper = FileMapper::new(dir, map);
    info!("Rfid player started");
    main_loop(audio_device, mfrc522, mapper);
}

fn main() {
    setup_signals();
    simple_logger::init().unwrap();
    log::set_max_level(log::LevelFilter::Info);
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
    match run(matches) {
        Err(e) => {
            error!("We caught an error: {}", e);
        }
        _ => {
            error!("We shouldn't have reached here");
        }
    }

}
