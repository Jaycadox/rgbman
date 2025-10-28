use anyhow::{Result, anyhow};
use i2c_linux::I2c;
use std::fs::File;

pub struct I2cDram {
    addresses: Vec<u16>,
    i2c: I2c<File>,
    led_count: u8,
}

impl I2cDram {
    fn register_read(i2c: &mut I2c<File>, reg: u16) -> Result<u8> {
        i2c.smbus_write_word_data(0x0, ((reg << 8) & 0xFF00) | ((reg >> 8) & 0x00FF))?;
        Ok(i2c.smbus_read_byte_data(0x81)?)
    }

    fn register_write(i2c: &mut I2c<File>, reg: u16, val: u8) -> Result<()> {
        i2c.smbus_write_word_data(0x0, ((reg << 8) & 0xFF00) | ((reg >> 8) & 0x00FF))?;
        i2c.smbus_write_byte_data(0x01, val)?;
        Ok(())
    }

    fn register_write_block(i2c: &mut I2c<File>, reg: u16, data: &[u8]) -> Result<()> {
        i2c.smbus_write_word_data(0x0, ((reg << 8) & 0xFF00) | ((reg >> 8) & 0x00FF))?;
        i2c.smbus_write_block_data(0x03, data)?;
        Ok(())
    }

    pub fn new(addresses: Vec<u16>) -> Result<I2cDram> {
        let mut i2c = I2c::from_path("/dev/i2c-2")?;
        for address in &addresses {
            i2c.smbus_set_slave_address(*address, false)?;
            i2c.smbus_read_byte()?;

            for i in 0xAD..0xB0 {
                let res = i2c.smbus_read_byte_data(i)?;
                if res != i - 0xA0 {
                    return Err(anyhow!("bad dram"));
                }
            }
            Self::register_write(&mut i2c, 0x8020, 1)?;
            Self::register_write(&mut i2c, 0x80A0, 1)?;
        }

        let mut config_table = [0; 64];
        for i in 0..64 {
            config_table[i as usize] = Self::register_read(&mut i2c, 0x1C00 + i)?;
        }
        let led_count = config_table[2];

        Ok(Self {
            addresses,
            i2c,
            led_count,
        })
    }

    pub fn set_led_colour(&mut self, r: u8, g: u8, b: u8) -> Result<()> {
        for address in &self.addresses {
            self.i2c.smbus_set_slave_address(*address, false)?;
            Self::register_write(&mut self.i2c, 0x8020, 1)?;
            Self::register_write(&mut self.i2c, 0x80A0, 1)?;
            for i in 0..self.led_count {
                Self::register_write_block(&mut self.i2c, 0x8100 + (3 * u16::from(i)), &[r, b, g])?;
            }
        }
        Ok(())
    }
}
