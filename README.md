# Game Boy RW, host side

This is the host side of my Game Boy reader/writer.

- [gb-rw-firm](https://github.com/Dhole/gb-rw-firm): Firmware part of the program (also written in Rust!).
- [gb-cart-nucleo-shield](https://github.com/Dhole/gb-cart-nucleo-shield): PCB
  design to beused on top of a STM32 Nucleo compatible development board.

## Supported features:

- Reading zipped ROM files to write
- GB
    - Reading ROM
    - Reading/Writing SRAM
    - Flashing ROM on flash Chinese cartridges
- GBA
    - Reading ROM
    - Flashing ROM on flash Chinese cartridges

## Details

You can read more about how reprogramming Game Boy Chinese cartridge works in my blog post:

- [Writing Game Boy Chinese cartridges with an STM32F4](https://dhole.github.io/post/gameboy_cartridge_rw_1/)

This program also uses my [`uRPC` library](https://github.com/Dhole/urpc)
(client side) to communicate with the microcontroller via UART.

## Dependencies

The reset functionality (enabled by default, disabled with the global flag
`--no-reset`) requires the comand line utility `st-flash`.

Debian install:
```
sudo apt install stlink-tools
```

Arch install:
```
sudo pacman -S stlink
```

## Usage

Main subcommands
```
Gameboy cartridge read/writer

USAGE:
    gb-rw-host [FLAGS] [OPTIONS] [SUBCOMMAND]

FLAGS:
    -h, --help        Prints help information
    -n, --no-reset    Don't reset the development board
    -V, --version     Prints version information

OPTIONS:
    -b, --baud <RATE>        Set the baud rate [default: 1500000]
    -d, --board <BOARD>      Set the development board: generic, st [default: st]
    -s, --serial <DEVICE>    Set the serial device [default: /dev/ttyACM0]

SUBCOMMANDS:
    debug    Device debug functions
    gb       Gameboy functions
    gba      Gameboy Advance functions
    help     Prints this message or the help of the given subcommand(s)
```

Gameboy subcommands
```
Gameboy functions

USAGE:
    gb-rw-host gb [SUBCOMMAND]

FLAGS:
    -h, --help       Prints help information
    -V, --version    Prints version information

SUBCOMMANDS:
    erase         erase Gameboy Flash ROM
    flash-info    Gameboy Flash chip info
    help          Prints this message or the help of the given subcommand(s)
    read          read Gameboy ROM test
    read-ram      read Gameboy RAM
    read-rom      read Gameboy ROM
    write-ram     write Gameboy RAM
    write-rom     write Gameboy Flash ROM
```

Gameboy Advance subcommands
```
Gameboy Advance functions

USAGE:
    gb-rw-host gba [SUBCOMMAND]

FLAGS:
    -h, --help       Prints help information
    -V, --version    Prints version information

SUBCOMMANDS:
    help          Prints this message or the help of the given subcommand(s)
    read-rom      read Gameboy Advance ROM
    write-rom     write Gameboy Advance ROM
```

## Quirks

When I started this project, I wrote the firmware in C.  For better
reliability, I added a global flag (enabled by default) to reset the
microcontroller before running the subcommand itself.

Afterwards I rewrote the firmware in Rust, and updated the host program
accordingly.  Currently, the reset feature doesn't play well with the firware,
so if you encounter any urpc errors, try disabling the resset by setting the
global flag `--no-reset`.

## License

The code is released under the 3-clause BSD License.
