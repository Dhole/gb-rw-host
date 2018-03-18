# Game Boy RW, host side

This is the host side of my Game Boy reader/writer.

Supported features are:

- Reading ROM
- Reading/Writing SRAM
- Writing ROM on flash Chinese cartridges
- Reading zipped ROM files to write

The code is written in Rust.

## Details

You can read more about how reprogramming Game Boy Chinese cartridge works in my blog post:

- [Writing Game Boy Chinese cartridges with an STM32F4](https://dhole.github.io/post/gameboy_cartridge_rw_1/)

## Usage

```
Gameboy cartridge read/writer

USAGE:
    gb-rw-host [OPTIONS] --file <FILE> --mode <MODE>

FLAGS:
    -h, --help       Prints help information
    -V, --version    Prints version information

OPTIONS:
    -b, --baud <RATE>        Set the baud rate [default: 1000000]
    -d, --board <BOARD>      Set the development board: generic, st [default: st]
    -f, --file <FILE>        Set the file to read/write for the cartridge ROM/RAM
    -m, --mode <MODE>        Set the operation mode: read_ROM, read_RAM, write_ROM, write_RAM
    -s, --serial <DEVICE>    Set the serial device [default: /dev/ttyACM0]
```

## License

The code is released under the 3-clause BSD License.
