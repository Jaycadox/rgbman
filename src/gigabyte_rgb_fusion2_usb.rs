use std::{ffi::CString, str::FromStr};

use anyhow::Result;
use hidapi::{HidApi, HidDevice, HidResult};

pub struct Fusion2Argb {
    dev: HidDevice,
    bo1: (u8, u8, u8),
    bo2: (u8, u8, u8),
    d1_count: usize,
    d2_count: usize,
    d3_count: usize,
    effect_mask: u8,
}

impl Fusion2Argb {
    const RID: u8 = 0xCC;

    const HDR_DLED1_RGB: u8 = 0x58;
    const HDR_DLED2_RGB: u8 = 0x59;
    const HDR_DLED3_RGB: u8 = 0x62;
    const LED3: u8 = 0x2;

    const LEDS_PER_PACKET: usize = 19;
    pub fn new() -> Result<Self> {
        let api = HidApi::new()?;
        let dev = api.open_path(CString::from_str("/dev/hidraw15").unwrap().as_c_str())?;
        Self::send64(&dev, 0x60, 0x00, 0x00)?;
        let rep = Self::get64(&dev)?;
        let (bo1, bo2) = Self::parse_byteorders(&rep);

        for reg in 0x20..=0x27 {
            Self::send64(&dev, reg, 0x00, 0x00)?;
        }
        Self::send64(&dev, 0x32, 0x00, 0x00)?; // clear disable bits
        Self::send64(&dev, 0x34, 0x00, 0x00)?; // clear counts
        for reg in 0x90..=0x92 {
            Self::send64(&dev, reg, 0x00, 0x00)?;
        }
        Self::send64(&dev, 0x28, 0xFF, 0x07)?; // IT5711 "apply" after reset
        Self::send64(&dev, 0x31, 0x00, 0x00)?; // beat off

        let d1_count = 60usize;
        let d2_count = 60usize;
        let d3_count = 60usize;
        Self::set_led_counts(&dev, d1_count, d2_count, d3_count)?;
        let mut x = Self {
            dev,
            bo1,
            bo2,
            d1_count,
            d2_count,
            d3_count,
            effect_mask: 0,
        };
        x.set_led_colour(0x00, 0x00, 0x00)?;
        Ok(x)
    }

    fn send64(dev: &HidDevice, a: u8, b: u8, c: u8) -> HidResult<()> {
        let mut buf = [0u8; 64];
        buf[0] = Self::RID;
        buf[1] = a;
        buf[2] = b;
        buf[3] = c;
        dev.send_feature_report(&buf)
    }

    fn get64(dev: &HidDevice) -> HidResult<[u8; 64]> {
        let mut buf = [0u8; 64];
        buf[0] = Self::RID;
        let _ = dev.get_feature_report(&mut buf)?;
        Ok(buf)
    }

    fn parse_byteorders(rep: &[u8; 64]) -> ((u8, u8, u8), (u8, u8, u8)) {
        let le32 = |i: usize| -> u32 {
            (rep[i] as u32)
                | ((rep[i + 1] as u32) << 8)
                | ((rep[i + 2] as u32) << 16)
                | ((rep[i + 3] as u32) << 24)
        };
        let bo1 = le32(44);
        let bo2 = le32(48);
        let to_trip = |bo: u32| -> (u8, u8, u8) {
            (
                ((bo >> 16) & 0xFF) as u8,
                ((bo >> 8) & 0xFF) as u8,
                (bo & 0xFF) as u8,
            )
        };
        let b1 = to_trip(bo1);
        let b2 = to_trip(bo2);
        let valid =
            |(r, g, b): (u8, u8, u8)| r <= 2 && g <= 2 && b <= 2 && r != g && g != b && r != b;
        (
            if valid(b1) { b1 } else { (0, 1, 2) },
            if valid(b2) { b2 } else { (0, 1, 2) },
        )
    }

    fn led_count_to_enum(count: usize) -> u8 {
        if count <= 32 {
            0
        } else if count <= 64 {
            1
        } else if count <= 256 {
            2
        } else if count <= 512 {
            3
        } else {
            4
        }
    }

    fn set_led_counts(dev: &HidDevice, d1: usize, d2: usize, d3: usize) -> HidResult<()> {
        let e1 = Self::led_count_to_enum(d1);
        let e2 = Self::led_count_to_enum(d2);
        let e3 = Self::led_count_to_enum(d3);
        Self::send64(dev, 0x34, (e2 << 4) | e1, e3)?;
        Ok(())
    }

    fn stream_strip_solid(
        dev: &HidDevice,
        hdr: u8,
        (bo_r, bo_g, bo_b): (u8, u8, u8),
        num_leds: usize,
        (r, g, b): (u8, u8, u8),
    ) -> HidResult<()> {
        let mut offset_bytes = 0usize;
        let mut leds_left = num_leds;

        while leds_left > 0 {
            let leds_in_pkt = leds_left.min(Self::LEDS_PER_PACKET);
            let bcount = (leds_in_pkt * 3) as u8;

            let mut buf = [0u8; 64];
            buf[0] = Self::RID;
            buf[1] = hdr;
            buf[2] = (offset_bytes & 0xFF) as u8; // boffset_lo
            buf[3] = ((offset_bytes >> 8) & 0xFF) as u8; // boffset_hi
            buf[4] = bcount;

            for i in 0..leds_in_pkt {
                let base = 5 + i * 3;
                buf[base + bo_r as usize] = r;
                buf[base + bo_g as usize] = g;
                buf[base + bo_b as usize] = b;
            }

            dev.send_feature_report(&buf)?;
            offset_bytes += leds_in_pkt * 3;
            leds_left -= leds_in_pkt;
        }

        Ok(())
    }

    pub fn set_led_colour(self: &mut Self, r: u8, g: u8, b: u8) -> Result<()> {
        self.effect_mask |= 0x01 | 0x02 | 0x08 | 0x10;
        self.effect_mask |= 0x10;
        Self::stream_strip_solid(
            &self.dev,
            Self::HDR_DLED3_RGB,
            self.bo2,
            self.d3_count,
            (r, g, b),
        )?;
        Self::send64(&self.dev, 0x32, self.effect_mask, 0x00)?;
        self.effect_mask |= 0x02;
        Self::stream_strip_solid(
            &self.dev,
            Self::HDR_DLED2_RGB,
            self.bo2,
            self.d2_count,
            (r, g, b),
        )?;
        Self::send64(&self.dev, 0x32, self.effect_mask, 0x00)?;
        self.effect_mask |= 0x1;
        Self::stream_strip_solid(
            &self.dev,
            Self::HDR_DLED1_RGB,
            self.bo1,
            self.d1_count,
            (r, g, b),
        )?;
        Self::send64(&self.dev, 0x32, self.effect_mask, 0x00)?;
        Self::stream_strip_solid(&self.dev, Self::LED3, self.bo1, 1, (r, g, b))?;
        Self::send64(&self.dev, 0x32, self.effect_mask, 0x00)?;

        let pkt = PktEffect::for_led(Self::LED3 as i32, 0x5711, Self::RID)
            .with_effect_type(effect::STATIC)
            .with_brightness(0xFF, 0x00)
            .with_color0_rgb(r, g, b)
            .to_bytes();

        self.dev.send_feature_report(&pkt)?;

        Self::send64(&self.dev, 0x28, 0xff, 0x00)?;
        Ok(())
    }
}

mod effect {
    pub const STATIC: u8 = 1;
}

#[derive(Clone)]
struct PktEffect {
    report_id: u8,
    header: u8,
    zone0: u32,
    zone1: u32,
    reserved0: u8,
    effect_type: u8,
    max_brightness: u8,
    min_brightness: u8,
    color0: u32,
    color1: u32,
    period0: u16,
    period1: u16,
    period2: u16,
    period3: u16,
    effect_param0: u8,
    effect_param1: u8,
    effect_param2: u8,
    effect_param3: u8,
}

impl PktEffect {
    pub fn for_led(led_index: i32, pid: u16, report_id: u8) -> Self {
        let mut pkt = Self::blank();
        pkt.report_id = report_id;

        if led_index < 0 {
            pkt.header = 0x20;
            pkt.zone0 = if pid == 0x5711 { 0x07FF } else { 0x00FF };
        } else if led_index < 8 {
            pkt.header = (0x20 + led_index as u8) & 0xFF;
            pkt.zone0 = 1u32 << (led_index as u32);
        } else if led_index < 11 {
            pkt.header = 0x90 + (led_index as u8 - 8);
            pkt.zone0 = 1u32 << (led_index as u32);
        } else {
            pkt.header = 0;
            pkt.zone0 = 0;
        }

        pkt
    }

    fn blank() -> Self {
        Self {
            report_id: 0x00,
            header: 0x00,
            zone0: 0,
            zone1: 0,
            reserved0: 0,
            effect_type: effect::STATIC,
            max_brightness: 255,
            min_brightness: 0,
            color0: 0,
            color1: 0,
            period0: 0,
            period1: 0,
            period2: 0,
            period3: 0,
            effect_param0: 0,
            effect_param1: 0,
            effect_param2: 0,
            effect_param3: 0,
        }
    }

    pub fn with_effect_type(mut self, ty: u8) -> Self {
        self.effect_type = ty;
        self
    }

    pub fn with_brightness(mut self, max: u8, min: u8) -> Self {
        self.max_brightness = max;
        self.min_brightness = min;
        self
    }

    pub fn with_color0_rgb(mut self, r: u8, g: u8, b: u8) -> Self {
        self.color0 = ((r as u32) << 16) | ((g as u32) << 8) | (b as u32);
        self
    }

    pub fn to_bytes(&self) -> [u8; 64] {
        let mut buf = [0u8; 64];

        let put_u16 = |b: &mut [u8], off: usize, v: u16| {
            b[off] = (v & 0xFF) as u8;
            b[off + 1] = (v >> 8) as u8;
        };
        let put_u32 = |b: &mut [u8], off: usize, v: u32| {
            b[off] = (v & 0xFF) as u8;
            b[off + 1] = ((v >> 8) & 0xFF) as u8;
            b[off + 2] = ((v >> 16) & 0xFF) as u8;
            b[off + 3] = ((v >> 24) & 0xFF) as u8;
        };

        buf[0] = self.report_id;
        buf[1] = self.header;
        put_u32(&mut buf, 2, self.zone0);
        put_u32(&mut buf, 6, self.zone1);
        buf[10] = self.reserved0;
        buf[11] = self.effect_type;
        buf[12] = self.max_brightness;
        buf[13] = self.min_brightness;
        put_u32(&mut buf, 14, self.color0);
        put_u32(&mut buf, 18, self.color1);
        put_u16(&mut buf, 22, self.period0);
        put_u16(&mut buf, 24, self.period1);
        put_u16(&mut buf, 26, self.period2);
        put_u16(&mut buf, 28, self.period3);
        buf[30] = self.effect_param0;
        buf[31] = self.effect_param1;
        buf[32] = self.effect_param2;
        buf[33] = self.effect_param3;
        buf
    }
}
