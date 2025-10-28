use anyhow::{Context, Result};
use hidapi::{HidApi, HidDevice, HidResult};

pub struct Fusion2Argb {
    dev: HidDevice,
    effect_mask: u8,
}

impl Fusion2Argb {
    const RID: u8 = 0xCC;

    pub fn new() -> Result<Self> {
        let api = HidApi::new()?;
        let wanted_dev = api
            .device_list()
            .find(|x| {
                x.vendor_id() == 0x48D && x.product_id() == 0x5711 && x.usage_page() == 0xFF89
            })
            .context("failed to find Gigabyte RGB Fusion 2 USB device")?;
        let dev = api.open_path(wanted_dev.path())?;
        Self::send64(&dev, 0x60, 0x00, 0x00)?;

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

        Ok(Self {
            dev,
            effect_mask: 0,
        })
    }

    fn send64(dev: &HidDevice, a: u8, b: u8, c: u8) -> HidResult<()> {
        let mut buf = [0u8; 64];
        buf[0] = Self::RID;
        buf[1] = a;
        buf[2] = b;
        buf[3] = c;
        dev.send_feature_report(&buf)
    }

    pub fn set_led_colour(&mut self, r: u8, g: u8, b: u8) -> Result<()> {
        self.effect_mask |= 0x01 | 0x02 | 0x08 | 0x10;
        self.effect_mask |= 0x10;
        let pkt = PktEffect::all_leds(Self::RID)
            .with_effect_type(effect::STATIC)
            .with_brightness(0xFF, 0xFF)
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
    pub fn all_leds(report_id: u8) -> Self {
        let mut pkt = Self::blank();
        pkt.report_id = report_id;

        pkt.header = 0x20;
        pkt.zone0 = 0x07FF;
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
        self.color0 = ((u32::from(r)) << 16) | ((u32::from(g)) << 8) | (u32::from(b));
        self.color1 = 0x00ff_ffff;
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
