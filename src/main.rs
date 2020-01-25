extern crate hex;
extern crate linux_embedded_hal as hal;
extern crate mfrc522;
extern crate rodio;

use std::io::BufReader;

use hal::spidev::SpidevOptions;
use hal::sysfs_gpio::Direction;
use hal::{Pin, Spidev};
use mfrc522::Mfrc522;
use std::thread;
use std::time::Duration;
fn setup_rfid_reader() -> Mfrc522<Spidev, Pin> {
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

    println!("VERSION: 0x{:x}", vers);

    assert!(vers == 0x91 || vers == 0x92);
    mfrc522
}

fn main() {
    let mut mfrc522 = setup_rfid_reader();
    let mut playing: Option<String> = None;

    let device = rodio::default_output_device().unwrap();
    let mut current_sink: Option<rodio::Sink> = None;

    loop {
        if let Ok(uid) = mfrc522.reqa().and_then(|atqa| mfrc522.select(&atqa)) {
            let encoded_id = hex::encode(uid.bytes());
            if Some(&encoded_id) != playing.as_ref() {
                let fname = format!("music/{}.mp3", encoded_id);
                match std::fs::File::open(&fname) {
                    Ok(opened_file) => {
                        if let Ok(new_sink) = rodio::play_once(&device, BufReader::new(opened_file))
                        {
                            let old_sink = current_sink.replace(new_sink);
                            if let Some(sink) = old_sink {
                                sink.stop();
                            }
                            println!("Playing {}", fname);
                        }
                    }
                    Err(error) => {
                        println!("Error opening {}: {}", fname, error);
                    }
                }
                playing.replace(encoded_id);
            }
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
