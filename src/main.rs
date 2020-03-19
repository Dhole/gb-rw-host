#![allow(dead_code)]
#![allow(unused_variables)]

#[macro_use]
extern crate urpc;

#[macro_use]
extern crate enum_primitive;
extern crate bufstream;
extern crate clap;
extern crate num;
extern crate pbr;
extern crate serial;
extern crate time;
extern crate zip;

extern crate gb_rw_host;

use std::io;
use std::io::ErrorKind;
// use std::io::{BufReader, BufWriter};
use std::ffi::OsStr;
use std::fs;
use std::fs::File;
use std::fs::OpenOptions;
use std::path::Path;
use std::process;
use std::process::Command;
use std::thread;
use std::time::Duration;

use urpc::{client, consts, OptBufNo, OptBufYes};

use bufstream::BufStream;
use clap::{App, Arg, ArgMatches, SubCommand};
use num::iter::range_step;
use pbr::{ProgressBar, Units};
use serial::prelude::*;
use std::io::prelude::*;
use zip::read::ZipArchive;
use zip::result::ZipError;

use gb_rw_host::header::*;
use gb_rw_host::utils::*;

#[derive(Debug)]
pub enum Error {
    Serial(io::Error),
    Zip(ZipError),
    Generic(String),
}

impl From<io::Error> for Error {
    fn from(err: io::Error) -> Self {
        Error::Serial(err)
    }
}

impl From<ZipError> for Error {
    fn from(err: ZipError) -> Self {
        Error::Zip(err)
    }
}

#[derive(Debug, PartialEq)]
enum Board {
    Generic,
    St,
}

#[derive(Debug, PartialEq)]
enum Memory {
    Rom,
    Ram,
}

mod rpc {
    rpc_client_io! {
        Client;
        client_requests;
        (0, ping, Ping([u8; 4], OptBufNo, [u8; 4], OptBufNo)),
        (1, send_bytes, SendBytes((), OptBufYes, (), OptBufNo)),
        (2, add, Add((u8, u8), OptBufNo, u8, OptBufNo)),
        (3, recv_bytes, RecvBytes((), OptBufNo, (), OptBufYes)),
        (4, send_recv, SendRecv((), OptBufYes, (), OptBufYes)),
        (5, gb_read, GBRead((u16, u16), OptBufNo, (), OptBufYes)),
        (6, gb_mode, GBMode((), OptBufNo, bool, OptBufNo)),
        (7, gb_write_word, GBWriteWord((u16, u8), OptBufNo, (), OptBufNo))
    }
}

// struct RpcClientIO<S: io::Read + io::Write> {
//     client: client::RpcClient,
//     stream: S,
//     pub stream_buf: Vec<u8>,
//     pub body_buf: Option<Vec<u8>>,
//     pub opt_buf: Option<Vec<u8>>,
// }
//
// #[derive(Debug)]
// enum RpcError {
//     Io(io::Error),
//     Urpc(urpc::client::Error),
// }
//
// impl From<io::Error> for RpcError {
//     fn from(err: io::Error) -> Self {
//         Self::Io(err)
//     }
// }
//
// impl From<urpc::client::Error> for RpcError {
//     fn from(err: urpc::client::Error) -> Self {
//         Self::Urpc(err)
//     }
// }
//
// impl<S: Read + Write> RpcClientIO<S> {
//     pub fn new(stream: S, buf_len: usize) -> Self {
//         Self {
//             client: client::RpcClient::new(),
//             stream: stream,
//             stream_buf: vec![0; buf_len],
//             body_buf: Some(vec![0; buf_len]),
//             opt_buf: Some(vec![0; buf_len]),
//         }
//     }
//
//     fn request(&mut self, chan_id: u8, write_len: usize) -> Result<(), RpcError> {
//         self.stream.write_all(&self.stream_buf[..write_len])?;
//         self.stream.flush()?;
//
//         let mut pos = 0;
//         let mut read_len = consts::REP_HEADER_LEN;
//         loop {
//             let mut buf = &mut self.stream_buf[..read_len];
//             self.stream.read_exact(&mut buf)?;
//             read_len = match self.client.parse(&buf)? {
//                 (n, None) => n,
//                 (n, Some(_chan_id)) => {
//                     if _chan_id == chan_id {
//                         return Ok(());
//                     }
//                     n
//                 }
//             }
//         }
//     }
// }
//
// struct RpcClient<S: io::Read + io::Write> {
//     rpc: RpcClientIO<S>,
// }
//
// impl<S: Read + Write> RpcClient<S> {
//     pub fn new(stream: S, buf_len: usize) -> Self {
//         Self {
//             rpc: RpcClientIO::new(stream, buf_len),
//         }
//     }
//     pub fn add(&mut self, arg: (u8, u8)) -> Result<u8, RpcError> {
//         let mut req = rpc::Add::new(arg);
//         let write_len = req.request(
//             &mut self.rpc.client,
//             self.rpc.body_buf.take().unwrap(),
//             &mut self.rpc.stream_buf,
//         )?;
//         self.rpc.request(req.chan_id(), write_len)?;
//         let (r, body_buf) = req.take_reply(&mut self.rpc.client).unwrap()?;
//         self.rpc.body_buf = Some(body_buf);
//         Ok(r)
//     }
// }

fn main() {
    let arg_file = Arg::with_name("file")
        .help("Set the file to read/write for the cartridge ROM/RAM")
        .short("f")
        .long("file")
        .value_name("FILE")
        .takes_value(true)
        .required(true);
    let mut app = App::new("gb-rw")
        .version("0.1")
        .about("Gameboy cartridge read/writer")
        .author("Dhole")
        .arg(
            Arg::with_name("baud")
                .help("Set the baud rate")
                .short("b")
                .long("baud")
                .value_name("RATE")
                //.default_value("115200")
                .default_value("921600")
                // .default_value("1152000")
                //.default_value("2000000")
                .takes_value(true)
                .required(false)
                .validator(|baud| match baud.parse::<usize>() {
                    Ok(_) => Ok(()),
                    Err(e) => Err(format!("{}", e)),
                }),
        )
        .arg(
            Arg::with_name("serial")
                .help("Set the serial device")
                .short("s")
                .long("serial")
                .value_name("DEVICE")
                .default_value("/dev/ttyACM0")
                .takes_value(true)
                .required(false),
        )
        .arg(
            Arg::with_name("board")
                .help("Set the development board: generic, st")
                .short("d")
                .long("board")
                .value_name("BOARD")
                .default_value("st")
                .takes_value(true)
                .required(false)
                .validator(|board| match board.as_str() {
                    "generic" => Ok(()),
                    "st" => Ok(()),
                    board => Err(format!("Invalid development board: {}", board)),
                }),
        )
        .arg(
            Arg::with_name("no-reset")
                .help("Don't reset the development board")
                .short("n"),
        )
        .subcommands(vec![
            SubCommand::with_name("read_ROM")
                .about("read Gameboy ROM")
                .arg(arg_file.clone()),
            SubCommand::with_name("read_RAM")
                .about("read Gameboy RAM")
                .arg(arg_file.clone()),
            SubCommand::with_name("write_ROM")
                .about("write Gameboy Flash ROM")
                .arg(arg_file.clone()),
            SubCommand::with_name("write_RAM")
                .about("write Gameboy RAM")
                .arg(arg_file.clone()),
            SubCommand::with_name("read_GBA_ROM")
                .about("read Gameboy Advance ROM")
                .arg(arg_file.clone())
                .arg(
                    Arg::with_name("size")
                        .help("ROM size in MB")
                        .short("s")
                        .long("size")
                        .value_name("SIZE")
                        .takes_value(true)
                        .required(false)
                        .validator(|size| match size.parse::<u32>() {
                            Ok(_) => Ok(()),
                            Err(e) => Err(format!("{}", e)),
                        }),
                ),
            SubCommand::with_name("read_GBA_test").about("read Gameboy Advance ROM test"),
            SubCommand::with_name("write_GBA_test").about("write Gameboy Advance ROM test"),
            SubCommand::with_name("write_GBA_ROM_test")
                .about("write Gameboy Advance ROM test")
                .arg(arg_file.clone()),
            SubCommand::with_name("erase").about("erase Gameboy Flash ROM"),
            SubCommand::with_name("read")
                .about("read Gameboy ROM test")
                .arg(arg_file.clone()),
            SubCommand::with_name("new_test")
                .about("test urpc")
                .arg(arg_file.clone()),
        ]);
    let matches = app.clone().get_matches();
    if matches.subcommand_name() == None {
        app.print_help().unwrap();
        println!();
        return;
    }

    run_subcommand(matches).unwrap_or_else(|e| {
        println!("Error during operation: {:?}", e);
        process::exit(1);
    });
}

struct Device<T: SerialPort> {
    port: BufStream<T>,
    board: Board,
}

impl<T: SerialPort> Device<T> {
    fn new(port: T, board: Board) -> Self {
        Self {
            port: BufStream::new(port),
            board,
        }
    }

    fn port_clear(&mut self) -> Result<(), io::Error> {
        let timeout = self.port.get_ref().timeout();
        self.port
            .get_mut()
            .set_timeout(Duration::from_millis(100))?;
        let mut buf = [0; 16];
        loop {
            match self.port.read(&mut buf) {
                Ok(0) => break,
                Ok(_) => continue,
                Err(e) => {
                    if e.kind() == ErrorKind::TimedOut {
                        break;
                    }
                    return Err(e);
                }
            }
        }
        self.port.get_mut().set_timeout(timeout)?;
        Ok(())
    }

    fn reset(&mut self) -> Result<(), Error> {
        match self.board {
            Board::Generic => {
                println!("Press the reset button on the board");
            }
            Board::St => {
                println!("\nResetting board using st-flash utility...");
                let output = Command::new("st-flash").arg("reset").output()?;
                println!("{}", String::from_utf8_lossy(&output.stderr));
                if !output.status.success() {
                    return Err(Error::Generic(format!(
                        "st-flash returned with error code {:?}",
                        output.status.code()
                    )));
                }
            }
        }
        let mut buf = Vec::new();
        loop {
            self.port.read_until(b'\n', &mut buf)?;
            if buf == b"HELLO\n" {
                break;
            }
            buf.clear();
        }
        Ok(())
    }

    fn send(&mut self, buf: &[u8]) -> Result<(), Error> {
        self.port.write_all(buf)?;
        self.port.flush()?;
        Ok(())
    }

    fn recv(&mut self, buf: &mut [u8]) -> Result<(), Error> {
        self.port.read_exact(buf)?;
        self.port.flush()?;
        Ok(())
    }

    fn wait_cmd_ack(&mut self) -> Result<(), Error> {
        let mut ack = [0];
        self.recv(&mut ack[..])?;
        if ack[0] != 0x41 {
            return Err(Error::Generic(format!(
                "Byte recieved isn't ACK but {:02x}",
                ack[0]
            )));
        }
        Ok(())
    }

    fn mode_gba(&mut self) -> Result<(), Error> {
        self.send(&cmd_mode_gba())?;
        self.wait_cmd_ack()
    }
}

impl<T: SerialPort> io::Read for Device<T> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.port.read(buf)
    }
}

impl<T: SerialPort> io::Write for Device<T> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.port.write(buf)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.port.flush()
    }
}

struct Gb<T: SerialPort> {
    dev: Device<T>,
}

impl<T: SerialPort> Gb<T> {
    fn new(dev: Device<T>) -> Self {
        Self { dev }
    }

    fn read(&mut self, addr: u16, buf: &mut [u8]) -> Result<(), Error> {
        unimplemented!()
    }

    fn write(&mut self, addr: u16, buf: &[u8]) -> Result<(), Error> {
        unimplemented!()
    }

    fn switch_bank(&mut self, mem_controller: &MemController, bank: usize) -> Result<(), Error> {
        unimplemented!()
    }

    fn ram_enable(&mut self) -> Result<(), Error> {
        unimplemented!()
    }

    fn ram_disable(&mut self) -> Result<(), Error> {
        unimplemented!()
    }

    fn erase_flash(&mut self) -> Result<(), Error> {
        unimplemented!()
    }
}

struct Gba<T: SerialPort> {
    dev: Device<T>,
}

impl<T: SerialPort> Gba<T> {
    fn new(mut dev: Device<T>) -> Result<Self, Error> {
        dev.mode_gba()?;
        Ok(Self { dev })
    }

    fn read(&mut self, addr: u32, buf: &mut [u8]) -> Result<(), Error> {
        self.dev
            .send(&cmd_gba_read(addr, addr + buf.len() as u32))?;
        self.dev.recv(&mut buf[..])?;
        self.dev.wait_cmd_ack()
    }

    fn read_word(&mut self, addr: u32) -> Result<u16, Error> {
        let mut buf = [0; 2];
        self.read(addr, &mut buf[..])?;
        Ok(u16::from_le_bytes(buf))
    }

    fn write_word(&mut self, addr: u32, data: u16) -> Result<(), Error> {
        self.dev.send(&cmd_gba_write(addr, data))?;
        self.dev.wait_cmd_ack()
    }

    fn flash_unlock_sector(&mut self, flash_type: FlashType, sector: u32) -> Result<(), Error> {
        match flash_type {
            FlashType::F3 => {
                self.write_word(sector, 0xff)?;
                self.write_word(sector, 0x60)?;
                self.write_word(sector, 0xd0)?;
                self.write_word(sector, 0x90)?;

                while self.read_word(sector + 2)? & 0x03 != 0x00 {}
                Ok(())
            }
        }
    }

    fn flash_erase_sector(&mut self, flash_type: FlashType, sector: u32) -> Result<(), Error> {
        match flash_type {
            FlashType::F3 => {
                self.write_word(sector, 0xff)?;
                self.write_word(sector, 0x20)?;
                self.write_word(sector, 0xd0)?;
                while self.read_word(sector)? != 0x80 {
                    thread::sleep(Duration::from_millis(100));
                }
                self.write_word(sector, 0xff)?;
                Ok(())
            }
        }
    }

    fn flash_write(&mut self, flash_type: FlashType, addr: u32, data: &[u8]) -> Result<(), Error> {
        self.dev.send(&cmd_gba_flash_write(
            addr,
            addr + data.len() as u32,
            flash_type,
        ))?;
        self.dev.send(data)?;
        self.dev.wait_cmd_ack()
    }
}

enum_from_primitive! {
#[derive(Debug, PartialEq)]
enum FlashType {
    F3 = 3,
}
}

enum_from_primitive! {
#[derive(Debug, PartialEq)]
enum Cmd {
    Read = 0,
    Write = 1,
    WriteRaw = 2,
    WriteFlash = 3,
    Erase = 4,
    Reset = 5,
    Ping = 6,
    ModeGBA = 7,
    ModeGB = 8,
    ReadGBAROM = 9,
    WriteGBAROM = 10,
    WriteGBAFlash = 11,
    ReadGBAWord = 12,
}
}

enum_from_primitive! {
#[derive(Debug, PartialEq)]
enum CmdReply {
    DMAReady,
    DMANotReady,
    Pong
}
}

enum_from_primitive! {
#[derive(Debug, PartialEq)]
enum ReplyCmd {
    Ack,
    Nack,
}
}

fn cmd_gb_read(addr_start: u16, addr_end: u16) -> [u8; 5] {
    let start = addr_start.to_le_bytes();
    let end = addr_end.to_le_bytes();
    [Cmd::Read as u8, start[0], start[1], end[0], end[1]]
}

fn cmd_gb_write_byte(addr: u16, data: u8) -> [u8; 5] {
    let addr = addr.to_le_bytes();
    [Cmd::Write as u8, addr[0], addr[1], data, 0x00]
}

fn cmd_gb_flash_write(addr_start: u16, addr_end: u16) -> [u8; 5] {
    let start = addr_start.to_le_bytes();
    let end = addr_end.to_le_bytes();
    [Cmd::WriteFlash as u8, start[0], start[1], end[0], end[1]]
}

fn cmd_gb_write(addr_start: u16, addr_end: u16) -> [u8; 5] {
    let start = addr_start.to_le_bytes();
    let end = addr_end.to_le_bytes();
    [Cmd::WriteRaw as u8, start[0], start[1], end[0], end[1]]
}

fn cmd_ping() -> [u8; 1] {
    [Cmd::Ping as u8]
}

fn cmd_mode_gba() -> [u8; 1] {
    [Cmd::ModeGBA as u8]
}

fn cmd_gba_read(addr_start: u32, addr_end: u32) -> [u8; 7] {
    let start = (addr_start >> 1).to_le_bytes();
    // We send the last addr_end we ask for because otherwise for roms of 32MB we overflow the 24
    // bits
    let end = ((addr_end >> 1) - 1).to_le_bytes();
    [
        Cmd::ReadGBAROM as u8,
        start[0],
        start[1],
        start[2],
        end[0],
        end[1],
        end[2],
    ]
}

fn cmd_gba_read_word(addr: u32) -> [u8; 4] {
    let addr = (addr >> 1).to_le_bytes();
    [Cmd::ReadGBAWord as u8, addr[0], addr[1], addr[2]]
}

fn cmd_gba_write(addr: u32, data: u16) -> [u8; 6] {
    let addr = (addr >> 1).to_le_bytes();
    let data = data.to_le_bytes();
    [
        Cmd::WriteGBAROM as u8,
        addr[0],
        addr[1],
        addr[2],
        data[0],
        data[1],
    ]
}

fn cmd_gba_flash_write(addr_start: u32, addr_end: u32, flash_type: FlashType) -> [u8; 8] {
    let start = (addr_start >> 1).to_le_bytes();
    let end = ((addr_end >> 1) - 1).to_le_bytes();
    [
        Cmd::WriteGBAFlash as u8,
        start[0],
        start[1],
        start[2],
        end[0],
        end[1],
        end[2],
        flash_type as u8,
    ]
}

fn run_subcommand(matches: ArgMatches) -> Result<(), Error> {
    let serial = matches.value_of("serial").unwrap();
    let baud = matches.value_of("baud").unwrap().parse::<usize>().unwrap();
    let board = match matches.value_of("board").unwrap() {
        "generic" => Board::Generic,
        "st" => Board::St,
        board => panic!("Invalid board: {}", board),
    };
    let reset = !matches.is_present("no-reset");

    println!("Development board is: {:?}", board);
    println!("Using serial device: {} at baud rate: {}", serial, baud);

    let mut port_raw = match serial::open(serial) {
        Ok(port) => port,
        Err(e) => {
            println!("Error opening {}: {}", serial, e);
            process::exit(1);
        }
    };
    port_raw
        .configure(&serial::PortSettings {
            baud_rate: serial::BaudRate::from_speed(baud),
            char_size: serial::Bits8,
            parity: serial::ParityNone,
            stop_bits: serial::Stop1,
            flow_control: serial::FlowNone,
        })
        .unwrap_or_else(|e| {
            println!("Error configuring {}: {}", serial, e);
            process::exit(1);
        });
    port_raw
        .set_timeout(Duration::from_secs(16))
        .unwrap_or_else(|e| {
            println!("Error setting timeout for {}: {}", serial, e);
            process::exit(1);
        });
    let mut device = Device::new(port_raw, board);
    device.port_clear()?;

    if reset {
        device.reset().unwrap_or_else(|e| {
            println!("Error resetting development board: {:?}", e);
            process::exit(1);
        });
        println!("Connected!");
    }

    let path = Path::new(match matches.subcommand() {
        ("erase", Some(_)) => "",
        ("read", Some(_)) => "",
        ("read_GBA_test", Some(_)) => "",
        ("write_GBA_test", Some(_)) => "",
        // ("new_test", Some(_)) => "",
        (_, Some(sub_m)) => sub_m.value_of("file").unwrap(),
        _ => unreachable!(),
    });

    // let mut rpc_client = client::RpcClient::new();
    let mut rpc_client = rpc::Client::new(device, 0x4000);

    let result = match matches.subcommand() {
        ("read_ROM", Some(_)) => {
            unimplemented!()
            // UPDATE
            // println!("Reading cartridge ROM into {}", path.display());
            // let file = OpenOptions::new().write(true).create_new(true).open(path)?;
            // read(&mut port, &file, Memory::Rom)
        }
        ("read_RAM", Some(_)) => {
            unimplemented!()
            // UPDATE
            // println!("Reading cartridge RAM into {}", path.display());
            // let file = OpenOptions::new().write(true).create_new(true).open(path)?;
            // read(&mut port, &file, Memory::Ram)
        }
        ("write_ROM", Some(_)) => {
            unimplemented!()
            // UPDATE
            // println!("Writing {} into cartridge ROM", path.display());
            // let mut rom = Vec::new();
            // if path.extension() == Some(OsStr::new("zip")) {
            //     let mut zip = ZipArchive::new(File::open(path)?)?;
            //     for i in 0..zip.len() {
            //         let mut zip_file = zip.by_index(i).unwrap();
            //         let filename = zip_file.name().to_string();
            //         let extension = Path::new(&filename).extension();
            //         if extension == Some(OsStr::new("gb")) || extension == Some(OsStr::new("gbc")) {
            //             zip_file.read_to_end(&mut rom)?;;
            //             break;
            //         }
            //     }
            //     if rom.len() == 0 {
            //         return Err(Error::Generic(format!(
            //             "File {} doesn't contain any ROM",
            //             path.display()
            //         )));
            //     }
            // } else {
            //     OpenOptions::new()
            //         .read(true)
            //         .open(path)?
            //         .read_to_end(&mut rom)?;
            // }
            // write_flash(&mut port, &rom)
        }
        ("write_RAM", Some(_)) => {
            unimplemented!()
            // UPDATE
            // println!("Writing {} into cartridge RAM", path.display());
            // let mut rom = Vec::new();
            // OpenOptions::new()
            //     .read(true)
            //     .open(path)?
            //     .read_to_end(&mut rom)?;
            // write_ram(&mut port, &rom)
        }
        ("read_GBA_ROM", Some(sub_m)) => {
            unimplemented!()
            // println!("Reading GBA cartridge ROM into {}", path.display());
            // let file = OpenOptions::new().write(true).create_new(true).open(path)?;
            // let size = sub_m.value_of("size").map(|s| s.parse::<u32>().unwrap());
            // gba_read_rom(&mut Gba::new(device)?, &file, size)
        }
        ("read_GBA_test", Some(_)) => {
            unimplemented!()
            // println!("Reading GBA cartridge ROM into stdout");
            // gba_read_test(&mut Gba::new(device)?)
        }
        ("write_GBA_test", Some(_)) => {
            unimplemented!()
            // println!("Writing and reading GBA cartridge data, showing result in stdout");
            // gba_write_test(&mut Gba::new(device)?)
        }
        ("write_GBA_ROM_test", Some(_)) => {
            unimplemented!()
            // println!("Writing {} into cartridge GBA ROM", path.display());
            // let mut rom = Vec::new();
            // OpenOptions::new()
            //     .read(true)
            //     .open(path)?
            //     .read_to_end(&mut rom)?;
            // write_gba_rom_test(&mut Gba::new(device)?, &rom)
        }
        ("new_test", Some(_)) => {
            // let r = rpc_client.ping([0, 1, 2, 3]).unwrap();
            // let r = rpc_client.send_bytes((), &[0, 1, 2, 3]).unwrap();
            // let r = rpc_client.add((3, 2)).unwrap();
            // let r = rpc_client.recv_bytes(()).unwrap();
            // let r = rpc_client.send_recv((), &[1, 2, 3, 4]).unwrap();
            //
            //println!("Reading cartridge ROM into {}", path.display());
            let file = OpenOptions::new().write(true).create_new(true).open(path)?;

            rpc_client.gb_mode(()).unwrap();
            println!("GB Mode Set");
            thread::sleep(Duration::from_millis(500));
            // let (_, buf) = rpc_client.gb_read((0x0000, 0x4000)).unwrap();
            // println!("reply: {:?}", buf);
            // print_hex(&buf, 0x0000);
            // Ok(())
            read(&mut rpc_client, &file, Memory::Rom)
        }
        // UPDATE
        // ("erase", Some(_)) => erase(&mut port),
        // UPDATE
        // ("read", Some(_)) => read_test(&mut port),
        (cmd, _) => {
            panic!("Unexpected subcommand: {}", cmd);
        }
    };
    println!();

    // Error cleanup
    if result.is_err() {
        match matches.subcommand_name() {
            Some("read_ROM") => fs::remove_file(path).unwrap_or(()),
            Some("read_RAM") => fs::remove_file(path).unwrap_or(()),
            Some("read_GBA_ROM") => fs::remove_file(path).unwrap_or(()),
            Some("new_test") => fs::remove_file(path).unwrap_or(()),
            _ => (),
        }
    }
    result
}

fn read<S: io::Read + io::Write>(
    mut rpc_client: &mut rpc::Client<S>,
    mut file: &File,
    memory: Memory,
) -> Result<(), Error> {
    // Read Bank 00
    println!("Reading ROM bank 000");
    let addr_start = 0x0000 as u16;
    let size = 0x4000 as u16;
    let (_, buf) = rpc_client.gb_read((addr_start, size)).unwrap();

    println!();
    //print_hex(&buf[0x0000..0x0200], 0x0000);
    //println!();

    let header_info = match parse_header(&buf) {
        Ok(header_info) => header_info,
        Err(e) => {
            return Err(Error::Generic(format!(
                "Error parsing cartridge header: {:?}",
                e
            )));
        }
    };

    let header_checksum = header_checksum(&buf);

    print_header(&header_info);
    println!();

    if header_info.checksum != header_checksum {
        return Err(Error::Generic(format!(
            "Header checksum mismatch: {:02x} != {:02x}",
            header_info.checksum, header_checksum
        )));
    }

    let mut mem = Vec::new();
    match memory {
        Memory::Rom => {
            mem.extend_from_slice(&buf);
            let addr_start = 0x4000 as u16;
            let size = 0x4000 as u16;
            for bank in 1..header_info.rom_banks {
                println!("Switching to ROM bank {:03}", bank);
                switch_bank(&mut rpc_client, &header_info.mem_controller, &memory, bank)?;
                println!("Reading ROM bank {:03}", bank);
                let (_, buf) = rpc_client.gb_read((addr_start, size)).unwrap();
                mem.extend_from_slice(&buf);
            }
            println!();
            let global_checksum = global_checksum(&mem);
            if header_info.global_checksum != global_checksum {
                println!(
                    "Global checksum mismatch: {:02x} != {:02x}",
                    header_info.global_checksum, global_checksum
                );
            } else {
                println!("Global checksum verification successfull!");
            }
        }
        Memory::Ram => {
            unimplemented!();
            // ram_enable(&mut port)?;
            // let addr_start = 0xA000 as u16;
            // let addr_end = if header_info.ram_size < 0x2000 {
            //     0xA000 + header_info.ram_size as u16
            // } else {
            //     0xC000 as u16
            // };
            // for bank in 0..(header_info.ram_banks + 1) {
            //     // Pipeline requests and reads
            //     if bank != header_info.ram_banks {
            //         println!("Switching to RAM bank {:03}", bank);
            //         switch_bank(&mut port, &header_info.mem_controller, &memory, bank)?;
            //         println!("Reading RAM bank {:03}", bank);
            //         port.write_all(cmd_gb_read(addr_start, addr_end).as_slice())?;
            //         port.flush()?;
            //     }
            //     if bank != 0 {
            //         let mut buf = vec![0; (addr_end - addr_start) as usize];
            //         port.read_exact(&mut buf)?;
            //         mem.extend_from_slice(&buf);
            //     }
            // }
            // println!();
            // ram_disable(&mut port)?;
        }
    }
    println!("Writing file...");
    file.write_all(&mem)?;
    Ok(())
}

// UPDATE
// fn read<T: SerialPort>(
//     mut port: &mut BufStream<T>,
//     mut file: &File,
//     memory: Memory,
// ) -> Result<(), Error> {
//     // Read Bank 00
//     println!("Reading ROM bank 000");
//     let addr_start = 0x0000 as u16;
//     let addr_end = 0x4000 as u16;
//     port.write_all(cmd_gb_read(addr_start, addr_end).as_slice())?;
//     port.flush()?;
//
//     let mut buf = vec![0; (addr_end - addr_start) as usize];
//     port.read_exact(&mut buf)?;
//
//     println!();
//     //print_hex(&buf[0x0000..0x0200], 0x0000);
//     //println!();
//
//     let header_info = match parse_header(&buf) {
//         Ok(header_info) => header_info,
//         Err(e) => {
//             return Err(Error::Generic(format!(
//                 "Error parsing cartridge header: {:?}",
//                 e
//             )));
//         }
//     };
//
//     let header_checksum = header_checksum(&buf);
//
//     print_header(&header_info);
//     println!();
//
//     if header_info.checksum != header_checksum {
//         return Err(Error::Generic(format!(
//             "Header checksum mismatch: {:02x} != {:02x}",
//             header_info.checksum, header_checksum
//         )));
//     }
//
//     let mut mem = Vec::new();
//     match memory {
//         Memory::Rom => {
//             mem.extend_from_slice(&buf);
//             let addr_start = 0x4000 as u16;
//             let addr_end = 0x8000 as u16;
//             for bank in 1..(header_info.rom_banks + 1) {
//                 // Pipeline requests and reads
//                 if bank != header_info.rom_banks {
//                     println!("Switching to ROM bank {:03}", bank);
//                     switch_bank(&mut port, &header_info.mem_controller, &memory, bank)?;
//                     println!("Reading ROM bank {:03}", bank);
//                     port.write_all(cmd_gb_read(addr_start, addr_end).as_slice())?;
//                     port.flush()?;
//                 }
//                 if bank != 1 {
//                     let mut buf = vec![0; (addr_end - addr_start) as usize];
//                     port.read_exact(&mut buf)?;
//                     mem.extend_from_slice(&buf);
//                 }
//             }
//             println!();
//             let global_checksum = global_checksum(&mem);
//             if header_info.global_checksum != global_checksum {
//                 println!(
//                     "Global checksum mismatch: {:02x} != {:02x}",
//                     header_info.global_checksum, global_checksum
//                 );
//             } else {
//                 println!("Global checksum verification successfull!");
//             }
//         }
//         Memory::Ram => {
//             ram_enable(&mut port)?;
//             let addr_start = 0xA000 as u16;
//             let addr_end = if header_info.ram_size < 0x2000 {
//                 0xA000 + header_info.ram_size as u16
//             } else {
//                 0xC000 as u16
//             };
//             for bank in 0..(header_info.ram_banks + 1) {
//                 // Pipeline requests and reads
//                 if bank != header_info.ram_banks {
//                     println!("Switching to RAM bank {:03}", bank);
//                     switch_bank(&mut port, &header_info.mem_controller, &memory, bank)?;
//                     println!("Reading RAM bank {:03}", bank);
//                     port.write_all(cmd_gb_read(addr_start, addr_end).as_slice())?;
//                     port.flush()?;
//                 }
//                 if bank != 0 {
//                     let mut buf = vec![0; (addr_end - addr_start) as usize];
//                     port.read_exact(&mut buf)?;
//                     mem.extend_from_slice(&buf);
//                 }
//             }
//             println!();
//             ram_disable(&mut port)?;
//         }
//     }
//     println!("Writing file...");
//     file.write_all(&mem)?;
//     Ok(())
// }

fn switch_bank<S: io::Read + io::Write>(
    mut rpc_client: &mut rpc::Client<S>,
    mem_controller: &MemController,
    memory: &Memory,
    bank: usize,
) -> Result<(), Error> {
    let mut writes: Vec<(u16, u8)> = Vec::new();
    match *mem_controller {
        MemController::None => {
            if bank != 1 {
                return Err(Error::Generic(format!(
                    "ROM Only cartridges can't select bank"
                )));
            }
        }
        MemController::Mbc1 => {
            match *memory {
                Memory::Rom => {
                    writes.push((0x6000, 0x00)); // Select ROM Banking Mode for upper two bits
                    writes.push((0x2000, (bank as u8) & 0x1f)); // Set bank bits 0..4
                    writes.push((0x4000, ((bank as u8) & 0x60) >> 5)); // Set bank bits 5,6
                }
                Memory::Ram => {
                    writes.push((0x6000, 0x01)); // Select RAM Banking Mode
                    writes.push((0x4000, (bank as u8) & 0x03)); // Set bank bits 0..4
                }
            }
        }
        MemController::Mbc5 => {
            match *memory {
                Memory::Rom => {
                    writes.push((0x2000, ((bank as u16) & 0x00ff) as u8)); // Set bank bits 0..7
                    writes.push((0x3000, (((bank as u16) & 0x0100) >> 8) as u8));
                    // Set bank bit 8
                }
                Memory::Ram => {
                    writes.push((0x4000, (bank as u8) & 0x0f)); // Set bank bits 0..4
                }
            }
        }
        ref mc => {
            return Err(Error::Generic(format!(
                "Error: Memory controller {:?} not implemented yet",
                mc
            )));
        }
    }
    for (addr, data) in writes {
        rpc_client.gb_write_word((addr, data)).unwrap();
    }
    Ok(())
}

// UPDATE
// fn ram_enable<T: SerialPort>(port: &mut BufStream<T>) -> Result<(), io::Error> {
//     port.write_all(cmd_gb_write_byte(0x0000, 0x0A).as_slice())?;
//     port.flush()
// }

// UPDATE
// fn ram_disable<T: SerialPort>(port: &mut BufStream<T>) -> Result<(), io::Error> {
//     port.write_all(cmd_gb_write_byte(0x0000, 0x00).as_slice())?;
//     port.flush()
// }

// UPDATE
// fn erase<T: SerialPort>(mut port: &mut BufStream<T>) -> Result<(), Error> {
//     erase_flash(&mut port)?;
//     Ok(())
// }

// UPDATE
// fn erase_flash<T: SerialPort>(mut port: &mut BufStream<T>) -> Result<(), Error> {
//     switch_bank(&mut port, &MemController::Mbc5, &Memory::Rom, 1)?;
//
//     let writes = vec![
//         (0x0AAA, 0xA9),
//         (0x0555, 0x56),
//         (0x0AAA, 0x80),
//         (0x0AAA, 0xA9),
//         (0x0555, 0x56),
//         (0x0AAA, 0x10), // All
//                         //(0x0000, 0x30), // First segment
//     ];
//
//     println!("Erasing flash, wait...");
//     for (addr, data) in writes {
//         port.write_all(cmd_gb_write_byte(addr, data).as_slice())?;
//     }
//     port.flush()?;
//
//     loop {
//         thread::sleep(Duration::from_millis(500));
//         port.write_all(cmd_gb_read(0x0000, 0x0001).as_slice())?;
//         port.flush()?;
//         let mut buf = vec![0; 1];
//         port.read_exact(&mut buf)?;
//         if buf[0] == 0xFF {
//             //println!("");
//             break;
//         }
//         if buf[0] != 0x4c && buf[0] != 0x08 {
//             return Err(Error::Generic(format!(
//                 "Received incorrect erasing status 0x{:02x}, check the cartridge connection",
//                 buf[0]
//             )));
//         }
//         //print!("{:02x} ", buf[0]);
//         //io::stdout().flush()?;
//     }
//     println!("OK!");
//
//     Ok(())
// }

// UPDATE
// fn read_test<T: SerialPort>(mut port: &mut BufStream<T>) -> Result<(), Error> {
//     switch_bank(&mut port, &MemController::Mbc5, &Memory::Rom, 1)?;
//
//     let addr_start = 0x0000 as u16;
//     let addr_end = 0x4000 as u16;
//     port.write_all(cmd_gb_read(addr_start, addr_end).as_slice())?;
//     port.flush()?;
//
//     let mut buf = vec![0; (addr_end - addr_start) as usize];
//     port.read_exact(&mut buf)?;
//
//     print_hex(&buf[0x0000..0x0200], 0x0000);
//     println!();
//
//     let addr_start = 0x4000 as u16;
//     let addr_end = 0x8000 as u16;
//     port.write_all(cmd_gb_read(addr_start, addr_end).as_slice())?;
//     port.flush()?;
//
//     let mut buf = vec![0; (addr_end - addr_start) as usize];
//     port.read_exact(&mut buf)?;
//
//     print_hex(&buf[0x0000..0x0200], 0x4000);
//     println!();
//     Ok(())
// }

fn gba_read_rom<T: SerialPort>(
    gba: &mut Gba<T>,
    mut file: &File,
    size: Option<u32>,
) -> Result<(), Error> {
    let size = match size {
        None => {
            let s = guess_gba_rom_size(gba)?;
            println!("Guessed ROM size: {}MB ({}MBits)", s, s * 8);
            s
        }
        Some(s) => s,
    };

    let buf_len = 0x8000 as u32;
    let mut buf = vec![0; buf_len as usize];
    let end: u32 = size * 1024 * 1024;
    let mut pb = ProgressBar::new(end as u64);
    pb.set_units(Units::Bytes);
    pb.format("[=> ]");
    for addr in range_step(0x00_00_00_00, end, buf_len) {
        gba.read(addr, &mut buf)?;
        file.write_all(&buf)?;
        pb.add(buf_len as u64);
    }
    pb.finish_print("Reading ROM finished");
    Ok(())
}

fn gba_read_test<T: SerialPort>(gba: &mut Gba<T>) -> Result<(), Error> {
    let buf_len = 0x100;
    let mut buf = vec![0; 0x800000];
    //for addr_start in range_step(0x1_00_00_00, 0x1_01_00_00, buf_len) {
    //for addr_start in range_step(0x0, 0x4000 * 1, buf_len) {
    let start = 0x0000;
    for addr_start in range_step(start, start + buf_len * 1, buf_len) {
        let addr_end = addr_start + buf_len;
        //// READ_GBA_ROM
        gba.read(addr_start, &mut buf[..(addr_end - addr_start) as usize])?;
        //// READ_GBA_WORD
        // for addr in (addr_start..addr_end).step_by(2) {
        //     run_cmd(port, cmd_gba_read_word(addr).as_slice())?;
        //     let mut word_le: [u8; 2] = [0, 0];
        //     port.read_exact(&mut word_le[..])?;
        //     wait_cmd_ack(port)?;
        //     buf[addr as usize] = word_le[0];
        //     buf[addr as usize + 1] = word_le[1];
        // }

        println!("=== {:08x} - {:08x} ===", addr_start, addr_end);
        // print_hex(&buf[addr_start as usize..addr_end as usize], addr_start);
        print_hex(&buf[..(addr_end - addr_start) as usize], addr_start);
        println!();
    }
    Ok(())
}

fn gba_write_test<T: SerialPort>(gba: &mut Gba<T>) -> Result<(), Error> {
    // port.write_all(cmd_gba_write(0x000000, 0xf0f0).as_slice())?;
    // port.flush()?;

    // port.write_all(cmd_gba_write(0x000154, 0x9898).as_slice())?;
    // port.flush()?;

    // port.write_all(cmd_gba_read(0x000040, 0x000042).as_slice())?;
    // port.flush()?;

    // port.read_exact(&mut buf)?;
    // println!("* read(0x000040) = {:02x}{:02x}", buf[0], buf[1]);

    // port.write_all(cmd_gba_write(0x000000, 0x00ff).as_slice())?;
    // port.flush()?;

    // port.write_all(cmd_gba_write(0x0000aa, 0x0098).as_slice())?;
    // port.flush()?;

    // port.write_all(cmd_gba_write(0x000000, 0xf0f0).as_slice())?;
    // port.flush()?;

    // port.write_all(cmd_gba_write(0x0000aa, 0x9898).as_slice())?;
    // port.flush()?;

    // port.write_all(cmd_gba_read(0x000020, 0x000022).as_slice())?;
    // port.flush()?;

    // port.read_exact(&mut buf)?;
    // println!("* read(0x000020) = {:02x}{:02x}", buf[0], buf[1]);

    // port.write_all(cmd_gba_write(0x000000, 0xf0f0).as_slice())?;
    // port.flush()?;
    // port.write_all(cmd_gba_write(0x000154, 0x9898).as_slice())?;
    // port.flush()?;

    // // *0x8000040 != 0x5152
    // // *0x8000020 != 0x5152

    // port.write_all(cmd_gba_write(0x000000, 0x00f0).as_slice())?;
    // port.flush()?;

    // port.write_all(cmd_gba_write(0x000000, 0x00ff).as_slice())?;
    // port.flush()?;
    // port.write_all(cmd_gba_write(0x000000, 0x0050).as_slice())?;
    // port.flush()?;
    // port.write_all(cmd_gba_write(0x000000, 0x0090).as_slice())?;
    // port.flush()?;

    // *0x8000000 == 0x8a

    // port.write_all(cmd_gba_write(0x000000, 0x00ff).as_slice())?;
    // port.flush()?;
    // port.write_all(cmd_gba_write(0x000000, 0x0090).as_slice())?;
    // port.flush()?;

    // *0x8000002 == 0x8810 -> 3

    // Disable ID mode
    // port.write_all(cmd_gba_write(0x000002, 0x00ff).as_slice())?;
    // port.flush()?;

    let sector = 0x0000;

    //// Erase 0x8000 bytes sector

    // Guess: Unlock sector
    /*
    println!("DBG Unlock sector");
    bus_gba_write(port, sector, 0xff)?;
    bus_gba_write(port, sector, 0x60)?;
    bus_gba_write(port, sector, 0xd0)?;
    bus_gba_write(port, sector, 0x90)?;

    for _ in 0..32 {
        let v = bus_gba_read_word(port, sector + 2)?;
        println!("1 read({:08x}) = {:04x}", sector + 2, v);
        if v & 0x03 == 0 {
            break;
        }
        thread::sleep(Duration::from_millis(50));
    }

    // Erase sector
    println!("DBG Erase sector");
    bus_gba_write(port, sector, 0xff)?;
    bus_gba_write(port, sector, 0x20)?;
    bus_gba_write(port, sector, 0xd0)?;

    loop {
        let v = bus_gba_read_word(port, sector)?;
        println!("2 read({:08x}) = {:04x}", sector, v);
        if v == 0x80 {
            break;
        }
        thread::sleep(Duration::from_millis(50));
    }

    bus_gba_write(port, sector, 0xff)?;
    */

    // gba_flash_a_unlock_sector(port, 0 * 0x8000)?;
    // gba_flash_a_erase_sector(port, 0 * 0x8000)?;
    // for i in 0..5 {
    //     gba_flash_a_unlock_sector(port, i * 0x8000)?;
    //     gba_flash_a_erase_sector(port, i * 0x8000)?;
    //     println!("Erased sector {:08x}", i * 0x8000);
    // }
    // let data = vec![0, 1];
    // for i in 0..4 {
    //     bus_gba_flash_write(port, 0x20000 + i * 0x8000, &data, FlashType::F3)?;
    // }

    // gba.flash_unlock_sector(FlashType::F3, 0xfc0000)?;
    // gba.flash_erase_sector(FlashType::F3, 0xfc0000)?;
    // gba.flash_unlock_sector(FlashType::F3, 0xfc1000)?;
    // gba.flash_erase_sector(FlashType::F3, 0xfc1000)?;

    //// Write data
    // let sector = 0x0000;

    //let data = [0xaa, 0xaa, 0xaa, 0xaa, 0xaa, 0xaa, 0xaa, 0xaa];
    //let data = [0x88, 0x88, 0x88, 0x88, 0x88, 0x88, 0x88, 0x88];
    // let data = [0xcc, 0xcc, 0xcc, 0xcc, 0xcc, 0xcc, 0xcc, 0xcc];
    // let data = [0xee, 0xee, 0xcc, 0xcc, 0xaa, 0xaa, 0x99, 0x99];
    // let data = [0x00, 0x0f, 0xf0, 0xff];
    // let data = [0xff, 0xf0, 0x0f, 0x00];
    // let data = [0xde, 0xad, 0xbe, 0xef];
    // let mut data = vec![0; 2];
    // for i in 0..data.len() {
    //     data[i] = i as u8;
    // }
    // let data = vec![0, 1];

    // let word = bus_gba_read_word(port, sector)?;
    // println!("Before: {:08x} : {:04x}", sector, word);

    // println!("DBG Write flash F3 data");
    // bus_gba_flash_write(port, sector, &data, FlashType::F3)?;

    // let word = bus_gba_read_word(port, sector)?;
    // println!("After: {:08x} : {:04x}", sector, word);
    /*
    for i in 0..data.len() / 2 {
        println!("Write byte");
        let addr = sector + (i * 2) as u32;
        bus_gba_write(port, addr, 0xff)?;
        bus_gba_write(port, addr, 0x70)?;

        loop {
            let v = bus_gba_read_word(port, addr)?;
            println!("3 read({:08x}) = {:04x}", addr, v);
            if v == 0x80 {
                break;
            }
            // thread::sleep(Duration::from_millis(50));
        }

        bus_gba_write(port, addr, 0xff)?;
        bus_gba_write(port, addr, 0x40)?;

        let w = (data[i * 2] as u16) | ((data[i * 2 + 1] as u16) << 8);
        println!("4 write({:08x}) = {:04x}", addr, w);
        bus_gba_write(port, addr, w)?;

        loop {
            let v = bus_gba_read_word(port, addr)?;
            println!("4 read({:08x}) = {:04x}", addr, v);
            if v == 0x80 {
                break;
            }
            // thread::sleep(Duration::from_millis(50));
        }
    }
    bus_gba_write(port, sector, 0xff)?;
    */

    // for (i = 0; i < 0x8000; i++) {
    //   *r2 = 0xff;
    //   *r2 = 0x70;

    //   while (*r2 != 0x80);

    //   *r2 = 0xff;
    //   *r2 = 0x40;

    //   *r2 = (u16) *r5;
    //   while (*r2 != 0x80);

    //   r2 +=2
    //   r5 +=2
    // }

    // *r2 = 0xff;

    Ok(())
}

fn write_gba_rom_test<T: SerialPort>(gba: &mut Gba<T>, rom: &[u8]) -> Result<(), Error> {
    let sec_len: usize = 0x8000;
    if rom.len() % sec_len != 0 {
        return Err(Error::Generic(format!(
            "Rom length is not multiple of sector length ({}), it's: {:?}",
            sec_len,
            rom.len()
        )));
    }

    /*
    let mut pb = ProgressBar::new(rom.len() as u64);
    pb.set_units(Units::Bytes);
    pb.format("[=> ]");
    for i in 0..rom.len() / sec_len {
        let sector = (i as u32) * (sec_len as u32);
        gba_flash_a_unlock_sector(port, sector)?;
        gba_flash_a_erase_sector(port, sector)?;
        pb.add(sec_len as u64);
    }
    pb.finish_print("Erasing ROM finished");
    // thread::sleep(Duration::from_millis(1000));
    */

    // let mut buf = vec![0; sec_len];
    let mut buf = vec![0; 0x100];
    // let mut buf2 = vec![0; 0x10];
    let mut pb = ProgressBar::new(rom.len() as u64);
    pb.set_units(Units::Bytes);
    pb.format("[=> ]");
    for i in 0..rom.len() / sec_len {
        let sector = (i as u32) * (sec_len as u32);
        // println!("DBG Unlock");
        // gba_flash_a_unlock_sector(port, sector)?; // DBG
        gba.flash_unlock_sector(FlashType::F3, sector)?;
        for attempt in 0..4 {
            if i < 4 || i % 4 == 0 {
                gba.flash_erase_sector(FlashType::F3, sector)?;
            }
            let rom_sector = &rom[sector as usize..sector as usize + sec_len];
            // if rom_sector.iter().all(|b| *b == 0x00) || rom_sector.iter().all(|b| *b == 0xff) {
            if rom_sector.iter().all(|b| *b == 0xff) {
                break;
            }
            // println!("DBG flash write");
            gba.flash_write(FlashType::F3, sector, rom_sector)?;
            gba.read(sector, &mut buf)?;
            if buf[..] == rom[sector as usize..sector as usize + buf.len()] {
                break;
            }
            println!(
                "Sector {:08x} check didn't pass, flashing it again...",
                sector
            );
            println!("=== Expected ===");
            print_hex(
                &rom[sector as usize..sector as usize + 0x100],
                sector as u32,
            );
            println!();
            println!("=== Got ===");
            print_hex(&buf[..0x100], sector as u32);
            println!();
            if attempt == 3 {
                return Err(Error::Generic(format!(
                    "Sector {:08x} check didn't pass after 4 write attempts",
                    sector
                )));
            }
        }
        pb.add(sec_len as u64);
        // Check that previous sectors were not erased!
        // for j in 0..i {
        //     let sector2 = (j as u32) * (sec_len as u32);
        //     bus_gba_read(port, sector2, sector2 + 0x10 as u32, &mut buf2)?;
        //     if buf2[..] != rom[sector2 as usize..sector2 as usize + 0x10] {
        //         println!(
        //             "While writing sector {:08x}, sector {:08x} was erased!",
        //             sector, sector2
        //         );
        //         println!("=== Expected ===");
        //         print_hex(
        //             &rom[sector2 as usize..sector2 as usize + 0x10],
        //             sector2 as u32,
        //         );
        //         println!();
        //         println!("=== Got ===");
        //         print_hex(&buf2[..0x10], sector2 as u32);
        //         println!();
        //     } else {
        //         println!("sector {:08x} is good!", sector2);
        //     }
        // }
    }
    pb.finish_print("Writing ROM finished");

    Ok(())
}

fn guess_gba_rom_size<T: SerialPort>(gba: &mut Gba<T>) -> Result<u32, Error> {
    let mut buf = vec![0; 0x08];
    for size in vec![32, 16, 8, 4] {
        for end in vec![size, size - size / 4] {
            let addr_end = end * 1024 * 1024;
            let addr_start = addr_end - 0x08;
            gba.read(addr_start, &mut buf)?;

            if buf != vec![0x00, 0x20, 0x00, 0x20, 0x00, 0x20, 0x00, 0x20] {
                return Ok(size);
            }
        }
    }
    Err(Error::Generic(format!("Unable to guess gba rom size")))
}

// UPDATE
// fn write_ram<T: SerialPort>(mut port: &mut BufStream<T>, sav: &[u8]) -> Result<(), Error> {
//     // Read Bank 00
//     println!("Reading ROM bank 000");
//     let addr_start = 0x0000 as u16;
//     let addr_end = 0x4000 as u16;
//     port.write_all(cmd_gb_read(addr_start, addr_end).as_slice())?;
//     port.flush()?;
//
//     let mut buf = vec![0; (addr_end - addr_start) as usize];
//     port.read_exact(&mut buf)?;
//
//     println!();
//
//     let header_info = match parse_header(&buf) {
//         Ok(header_info) => header_info,
//         Err(e) => {
//             return Err(Error::Generic(format!(
//                 "Error parsing cartridge header: {:?}",
//                 e
//             )));
//         }
//     };
//
//     let header_checksum = header_checksum(&buf);
//
//     if header_info.checksum != header_checksum {
//         return Err(Error::Generic(format!(
//             "Header checksum mismatch: {:02x} != {:02x}",
//             header_info.checksum, header_checksum
//         )));
//     }
//
//     if sav.len() != header_info.ram_size {
//         return Err(Error::Generic(format!(
//             "RAM size mismatch between header and file: {:02x} != {:02x}",
//             header_info.ram_size,
//             sav.len()
//         )));
//     }
//
//     ram_enable(&mut port)?;
//     let addr_start = 0xA000 as u16;
//     let addr_end = if header_info.ram_size < 0x2000 {
//         0xA000 + header_info.ram_size as u16
//     } else {
//         0xC000 as u16
//     };
//     let bank_len = (addr_end - addr_start) as usize;
//     println!("addr_end = {}", addr_end);
//     let mut reply = vec![0; 1];
//     for bank in 0..header_info.ram_banks {
//         if bank != 0 {
//             println!("Switching to ROM bank {:03}", bank);
//             switch_bank(&mut port, &header_info.mem_controller, &Memory::Ram, bank)?;
//         }
//
//         loop {
//             port.write_all(cmd_gb_write(addr_start, addr_end).as_slice())?;
//             port.flush()?;
//
//             port.read_exact(&mut reply)?;
//             if reply[0] == CmdReply::DMAReady as u8 {
//                 break;
//             }
//             thread::sleep(Duration::from_millis(100));
//         }
//         println!("Writing RAM bank {:03}...", bank);
//         port.write_all(&sav[bank * 0x2000..bank * 0x2000 + bank_len])?;
//         port.flush()?;
//     }
//
//     ram_disable(&mut port)?;
//
//     println!("Waiting for confirmation...");
//     port.write_all(cmd_ping().as_slice())?;
//     port.flush()?;
//     port.read_exact(&mut reply)?;
//     if reply[0] == CmdReply::Pong as u8 {
//         println!("OK!");
//         Ok(())
//     } else {
//         Err(Error::Generic(format!("Unexpected reply to ping")))
//     }
// }

// UPDATE
// fn write_flash<T: SerialPort>(
//     mut port: &mut BufStream<T>,
//     //mut file: &File,
//     rom: &[u8],
// ) -> Result<(), Error> {
//     let header_info = match parse_header(rom) {
//         Ok(header_info) => header_info,
//         Err(e) => {
//             return Err(Error::Generic(format!("Error parsing rom header: {:?}", e)));
//         }
//     };
//
//     println!("ROM header info:");
//     println!();
//     print_header(&header_info);
//     println!();
//
//     match header_info.mem_controller {
//         MemController::None => (),
//         MemController::Mbc1 => {
//             if header_info.rom_banks > 0x20 {
//                 return Err(Error::Generic(format!(
//                     "MBC1 is only MBC5-compatible with max {} banks, but this rom has {} banks",
//                     0x1f, header_info.rom_banks
//                 )));
//             }
//         }
//         MemController::Mbc5 => (),
//         ref mc => {
//             return Err(Error::Generic(format!("{:?} is not MBC5-compatible", mc)));
//         }
//     }
//
//     erase_flash(&mut port)?;
//     println!();
//
//     let mut reply = vec![0; 1];
//     for bank in 0..header_info.rom_banks {
//         if bank != 0 {
//             println!("Switching to ROM bank {:03}", bank);
//             switch_bank(&mut port, &MemController::Mbc5, &Memory::Rom, bank)?;
//         }
//         let addr_start = if bank == 0 { 0x0000 } else { 0x4000 };
//
//         loop {
//             port.write_all(cmd_gb_flash_write(addr_start, addr_start + 0x4000).as_slice())?;
//             port.flush()?;
//
//             port.read_exact(&mut reply)?;
//             if reply[0] == CmdReply::DMAReady as u8 {
//                 break;
//             }
//             thread::sleep(Duration::from_millis(100));
//         }
//         println!("Writing ROM bank {:03}...", bank);
//         port.write_all(&rom[bank * 0x4000..bank * 0x4000 + 0x4000])?;
//         port.flush()?;
//     }
//
//     println!("Waiting for confirmation...");
//     port.write_all(cmd_ping().as_slice())?;
//     port.flush()?;
//     port.read_exact(&mut reply)?;
//     if reply[0] == CmdReply::Pong as u8 {
//         println!("OK!");
//         Ok(())
//     } else {
//         Err(Error::Generic(format!("Unexpected reply to ping")))
//     }
// }
