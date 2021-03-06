#![deny(warnings)]
#![feature(phase, tuple_indexing)]

extern crate libc;
extern crate native;
#[cfg(test)]
extern crate quickcheck;
#[cfg(test)]
#[phase(plugin)]
extern crate quickcheck_macros;

use native::io::file::FileDesc;
use std::io::{FileAccess, IoError, IoResult, Read, ReadWrite, Write};

use termios::{FAILURE, Termios, SUCCESS};

mod termios;
#[cfg(test)]
mod socat;
#[cfg(test)]
mod test;

const O_NOCTTY: libc::c_int = 0x0100;

#[deriving(PartialEq)]
pub struct BlockingMode {
    /// The device will block until `bytes` are received
    pub bytes: u8,
    /// The device will block for at least `deciseconds` after each `read()` call
    pub deciseconds: u8,
}

pub struct SerialPort {
    fd: libc::c_int,
    file: FileDesc,
    termios: Termios,
}

impl SerialPort {
    /// Opens a serial `device` in "raw" mode
    pub fn open(device: &Path, access: FileAccess) -> IoResult<SerialPort> {
        let flags = match access {
            Read => libc::O_RDONLY,
            ReadWrite => libc::O_RDWR,
            Write => libc::O_WRONLY,
        } | O_NOCTTY;

        let fd = match device.with_c_str(|s| unsafe { libc::open(s, flags, 0) }) {
            FAILURE => return Err(IoError::last_error()),
            fd => fd,
        };

        let file = FileDesc::new(fd, true);

        let mut termios = Termios::new();

        match unsafe { termios::tcgetattr(fd, &mut termios) } {
            FAILURE => return Err(IoError::last_error()),
            SUCCESS => {},
            _ => unreachable!(),
        }

        unsafe { termios::cfmakeraw(&mut termios) };

        let sp = SerialPort { fd: fd, file: file, termios: termios };

        try!(sp.update());

        Ok(sp)
    }

    /// Returns the input and output baud rates
    #[cfg(target_os = "linux")]
    pub fn baud_rate(&self) -> IoResult<(BaudRate, BaudRate)> {
        let termios = try!(self.fetch());

        let input = termios.c_ispeed;
        let input = match FromPrimitive::from_u32(input) {
            None => panic!("unrecognized BaudRate value: {}", input),
            Some(input) => input,
        };

        let output = termios.c_ospeed;
        let output = match FromPrimitive::from_u32(output) {
            None => panic!("unrecognized BaudRate value: {}", output),
            Some(output) => output,
        };

        Ok((input, output))
    }

    /// Returns the input and output baud rates
    #[cfg(target_os = "macos")]
    pub fn baud_rate(&self) -> IoResult<(BaudRate, BaudRate)> {
        let termios = try!(self.fetch());

        let input = termios.c_ispeed;
        let input = match FromPrimitive::from_u64(input) {
            None => panic!("unrecognized BaudRate value: {}", input),
            Some(input) => input,
        };

        let output = termios.c_ospeed;
        let output = match FromPrimitive::from_u64(output) {
            None => panic!("unrecognized BaudRate value: {}", output),
            Some(output) => output,
        };

        Ok((input, output))
    }

    /// Returns the blocking mode used by the device
    pub fn blocking_mode(&self) -> IoResult<BlockingMode> {
        use termios::{VMIN, VTIME};

        Ok(BlockingMode {
            bytes: self.termios.c_cc[VMIN as uint],
            deciseconds: self.termios.c_cc[VTIME as uint],
        })
    }

    /// Returns the number of data bits used per character
    #[cfg(target_os = "linux")]
    pub fn data_bits(&self) -> IoResult<DataBits> {
        use termios::CSIZE;

        let bits = try!(self.fetch()).c_cflag & CSIZE;

        match FromPrimitive::from_u32(bits) {
            None => panic!("unrecognized DataBits value: {}", bits),
            Some(bits) => Ok(bits),
        }
    }

    /// Returns the number of data bits used per character
    #[cfg(target_os = "macos")]
    pub fn data_bits(&self) -> IoResult<DataBits> {
        use termios::CSIZE;

        let bits = try!(self.fetch()).c_cflag & CSIZE;

        match FromPrimitive::from_u64(bits) {
            None => panic!("unrecognized DataBits value: {}", bits),
            Some(bits) => Ok(bits),
        }
    }

    /// Returns the flow control used by the device
    pub fn flow_control(&self) -> IoResult<FlowControl> {
        use termios::{CRTSCTS, IXANY, IXOFF, IXON};

        let termios = try!(self.fetch());

        if termios.c_cflag & CRTSCTS != 0 {
            Ok(HardwareControl)
        } else if termios.c_iflag & (IXANY | IXOFF | IXON) == 0 {
            Ok(NoFlowControl)
        } else {
            Ok(SoftwareControl)
        }
    }

    /// Returns the bit parity used by the device
    pub fn parity(&self) -> IoResult<Parity> {
        use termios::{PARENB, PARODD};

        let termios = try!(self.fetch());

        match (termios.c_cflag & PARENB != 0, termios.c_cflag & PARODD != 0) {
            (true, true) => Ok(OddParity),
            (true, false) => Ok(EvenParity),
            (false, _) => Ok(NoParity),
        }
    }

    /// Changes the baud rate of the input/output or both directions
    pub fn set_baud_rate(&mut self, direction: Direction, rate: BaudRate) -> IoResult<()> {
        use termios::speed_t;

        match unsafe { match direction {
            BothDirections => termios::cfsetspeed(&mut self.termios, rate as speed_t),
            Input => termios::cfsetispeed(&mut self.termios, rate as speed_t),
            Output => termios::cfsetospeed(&mut self.termios, rate as speed_t),
        } } {
            FAILURE => Err(IoError::last_error()),
            SUCCESS => self.update(),
            _ => unreachable!(),
        }
    }

    /// Changes the blocking mode used by the device
    pub fn set_blocking_mode(&mut self, mode: BlockingMode) -> IoResult<()> {
        use termios::{VMIN, VTIME};

        self.termios.c_cc[VMIN as uint] = mode.bytes;
        self.termios.c_cc[VTIME as uint] = mode.deciseconds;

        self.update()
    }

    /// Changes the number of data bits per character
    #[cfg(target_os = "linux")]
    pub fn set_data_bits(&mut self, bits: DataBits) -> IoResult<()> {
        use termios::CSIZE;

        self.termios.c_cflag &= !CSIZE;
        self.termios.c_cflag |= bits as u32;

        self.update()
    }

    /// Changes the number of data bits per character
    #[cfg(target_os = "macos")]
    pub fn set_data_bits(&mut self, bits: DataBits) -> IoResult<()> {
        use termios::CSIZE;

        self.termios.c_cflag &= !CSIZE;
        self.termios.c_cflag |= bits as u64;

        self.update()
    }

    /// Changes the flow control used by the device
    pub fn set_flow_control(&mut self, flow: FlowControl) -> IoResult<()> {
        use termios::{CRTSCTS, IXANY, IXOFF, IXON};

        match flow {
            HardwareControl => {
                self.termios.c_cflag |= CRTSCTS;
                self.termios.c_iflag &= !(IXANY | IXOFF | IXON);
            } NoFlowControl => {
                self.termios.c_cflag &= !CRTSCTS;
                self.termios.c_iflag &= !(IXANY | IXOFF | IXON);
            } SoftwareControl => {
                self.termios.c_cflag &= !CRTSCTS;
                self.termios.c_iflag |= IXANY | IXOFF | IXON;
            }
        }

        self.update()
    }

    /// Changes the bit parity used by the device
    pub fn set_parity(&mut self, parity: Parity) -> IoResult<()> {
        use termios::{PARENB, PARODD};

        match parity {
            EvenParity => {
                self.termios.c_cflag |= PARENB;
                self.termios.c_cflag &= !PARODD;
            },
            NoParity => self.termios.c_cflag &= !PARENB,
            OddParity => self.termios.c_cflag |= PARENB | PARODD,
        }

        self.update()
    }

    /// Changes the number of stop bits per character
    pub fn set_stop_bits(&mut self, bits: StopBits) -> IoResult<()> {
        use termios::CSTOPB;

        match bits {
            Stop1 => self.termios.c_cflag &= !CSTOPB,
            Stop2 => self.termios.c_cflag |= CSTOPB,
        }

        self.update()
    }

    /// Returns the number of stop bits per character
    pub fn stop_bits(&self) -> IoResult<StopBits> {
        use termios::CSTOPB;

        if try!(self.fetch()).c_cflag & CSTOPB == 0 {
            Ok(Stop1)
        } else {
            Ok(Stop2)
        }
    }

    /// Fetches the current state of the termios structure
    fn fetch(&self) -> IoResult<Termios> {
        let mut termios = Termios::new();

        match unsafe { termios::tcgetattr(self.fd, &mut termios) } {
            FAILURE => Err(IoError::last_error()),
            SUCCESS => Ok(termios),
            _ => unreachable!(),
        }
    }

    /// Updates the underlying termios structure
    fn update(&self) -> IoResult<()> {
        use termios::TCSANOW;

        match unsafe { termios::tcsetattr(self.fd, TCSANOW, &self.termios) } {
            FAILURE => Err(IoError::last_error()),
            SUCCESS => Ok(()),
            _ => unreachable!(),
        }
    }
}

impl Reader for SerialPort {
    fn read(&mut self, buf: &mut [u8]) -> IoResult<uint> {
        match self.file.inner_read(buf) {
            Err(err) => Err(IoError::from_errno(err.code, true)),
            Ok(ret) => Ok(ret),
        }
    }
}

impl Writer for SerialPort {
    fn write(&mut self, buf: &[u8]) -> IoResult<()> {
        match self.file.inner_write(buf) {
            Err(err) => Err(IoError::from_errno(err.code, true)),
            Ok(_) => Ok(()),
        }
    }
}

#[cfg(target_os = "linux")]
#[deriving(FromPrimitive, PartialEq, Show)]
#[repr(u32)]
pub enum BaudRate {
    B0 = termios::B0,
    B50 = termios::B50,
    B75 = termios::B75,
    B110 = termios::B110,
    B134 = termios::B134,
    B150 = termios::B150,
    B200 = termios::B200,
    B300 = termios::B300,
    B600 = termios::B600,
    B1K2 = termios::B1200,
    B1K8 = termios::B1800,
    B2K4 = termios::B2400,
    B4K8 = termios::B4800,
    B9K6 = termios::B9600,
    B19K2 = termios::B19200,
    B38K4 = termios::B38400,
    B57K6 = termios::B57600,
    B115K2 = termios::B115200,
    B230K4 = termios::B230400,
    B460K8 = termios::B460800,
    B500K = termios::B500000,
    B576K = termios::B576000,
    B921K6 = termios::B921600,
    B1M = termios::B1000000,
    B1M152 = termios::B1152000,
    B1M5 = termios::B1500000,
    B2M = termios::B2000000,
    B2M5 = termios::B2500000,
    B3M = termios::B3000000,
    B3M5 = termios::B3500000,
    B4M = termios::B4000000,
}

#[cfg(target_os = "macos")]
#[deriving(FromPrimitive, PartialEq, Show)]
#[repr(u64)]
pub enum BaudRate {
    B0 = termios::B0,
    B50 = termios::B50,
    B75 = termios::B75,
    B110 = termios::B110,
    B134 = termios::B134,
    B150 = termios::B150,
    B200 = termios::B200,
    B300 = termios::B300,
    B600 = termios::B600,
    B1K2 = termios::B1200,
    B1K8 = termios::B1800,
    B2K4 = termios::B2400,
    B4K8 = termios::B4800,
    B7K2 = termios::B7200,
    B9K6 = termios::B9600,
    B14K4 = termios::B14400,
    B19K2 = termios::B19200,
    B28K8 = termios::B28800,
    B38K4 = termios::B38400,
    B57K6 = termios::B57600,
    B76K8 = termios::B76800,
    B115K2 = termios::B115200,
    B230K4 = termios::B230400,
}

#[cfg(target_os = "linux")]
#[deriving(FromPrimitive, PartialEq, Show)]
#[repr(u32)]
pub enum DataBits {
    Data5 = termios::CS5,
    Data6 = termios::CS6,
    Data7 = termios::CS7,
    Data8 = termios::CS8,
}

#[cfg(target_os = "macos")]
#[deriving(FromPrimitive, PartialEq, Show)]
#[repr(u64)]
pub enum DataBits {
    Data5 = termios::CS5,
    Data6 = termios::CS6,
    Data7 = termios::CS7,
    Data8 = termios::CS8,
}

pub enum Direction {
    BothDirections,
    Input,
    Output,
}

#[deriving(FromPrimitive, PartialEq, Show)]
pub enum FlowControl {
    HardwareControl,
    NoFlowControl,
    SoftwareControl,
}

#[deriving(FromPrimitive, PartialEq, Show)]
pub enum Parity {
    EvenParity,
    NoParity,
    OddParity,
}

#[deriving(FromPrimitive, PartialEq, Show)]
#[repr(u32)]
pub enum StopBits {
    Stop1,
    Stop2,
}
