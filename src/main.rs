use anyhow::Result;
use chrono::{Local, Timelike};
use http_body_util::Full;
use hyper::body::{Bytes, Incoming};
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Request, Response};
use hyper_util::rt::TokioIo;
use std::convert::Infallible;
use std::env;
use std::net::SocketAddr;
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::sync::broadcast;
use tracing::{Subscriber, error, info};
use tracing_subscriber::{EnvFilter, Registry, prelude::*};

mod auda0_e6k5_0101_dram;
mod gigabyte_rgb_fusion2_usb;
use auda0_e6k5_0101_dram as dram;
use gigabyte_rgb_fusion2_usb as fusion2;

#[derive(PartialEq, Clone, Debug)]
enum Mode {
    On,
    Off,
}

#[derive(PartialEq, Clone, Debug)]
enum State {
    Fading(Mode, f32),
    Turning(Mode),
    Idle(Mode),
    SetColourThen(f32, Box<State>),
}

fn init_tracing() {
    let filter = EnvFilter::new("rgbman=info");
    // Build either journald or fmt logger, boxed as trait object
    let subscriber: Box<dyn Subscriber + Send + Sync> = if env::var_os("JOURNAL_STREAM").is_some() {
        match tracing_journald::layer() {
            Ok(journald) => Box::new(Registry::default().with(filter).with(journald)),
            Err(_) => Box::new(
                Registry::default()
                    .with(filter)
                    .with(tracing_subscriber::fmt::layer()),
            ),
        }
    } else {
        Box::new(
            Registry::default()
                .with(filter)
                .with(tracing_subscriber::fmt::layer()),
        )
    };

    tracing::subscriber::set_global_default(subscriber).unwrap();
}

async fn run_rgb_server(new_led_state: &mut tokio::sync::watch::Receiver<f32>) -> Result<()> {
    info!("rgb server: starting...");
    let mut dram = dram::I2cDram::new(vec![0x71, 0x73])?;
    let mut fans = fusion2::Fusion2Argb::new()?;

    info!(message = "rgb server ready",);

    loop {
        new_led_state.changed().await?;
        let x = *new_led_state.borrow();
        fans.set_led_colour((54.0 * x) as u8, (28.0 * x) as u8, (15.0 * x) as u8)?;
        dram.set_led_colour((89.0 * x) as u8, (60.0 * x) as u8, (46.0 * x) as u8)?;
    }
}

async fn run_state_machine(
    starting_state: State,
    mut long_wait_when_idle: bool,
    send_rgb_value: tokio::sync::watch::Sender<f32>,
) -> Result<()> {
    let mut state = starting_state;
    loop {
        let old_state = state.clone();
        state = match state {
            State::SetColourThen(x, new_state) => {
                send_rgb_value.send(x)?;
                *new_state
            }
            State::Fading(mode, raw_x) => {
                let x = match mode {
                    Mode::On => 1.0 - raw_x,
                    Mode::Off => raw_x,
                };

                if raw_x > 0.08 {
                    if (raw_x - 1.0).abs() > 0.01 {
                        tokio::time::sleep(Duration::from_millis(80)).await;
                    }
                    State::SetColourThen(x * 0.2, Box::new(State::Fading(mode, raw_x * 0.2)))
                } else {
                    State::Turning(mode)
                }
            }
            State::Turning(mode) => match mode {
                Mode::On => State::SetColourThen(1.0, Box::new(State::Idle(mode))),
                Mode::Off => State::SetColourThen(0.0, Box::new(State::Idle(mode))),
            },
            State::Idle(mode) => {
                if long_wait_when_idle {
                    tokio::time::sleep(Duration::from_secs(60 * 60 * 3)).await;
                    long_wait_when_idle = false;
                } else {
                    tokio::time::sleep(Duration::from_secs(5 * 60)).await;
                }
                let hour = Local::now().hour();
                if (8..18).contains(&hour) {
                    if mode == Mode::Off {
                        State::Fading(Mode::On, 1.0)
                    } else {
                        tokio::time::sleep(Duration::from_secs(5 * 60)).await;
                        State::Turning(Mode::On)
                    }
                } else if hour >= 18 {
                    if mode == Mode::On {
                        State::Fading(Mode::Off, 1.0)
                    } else {
                        tokio::time::sleep(Duration::from_secs(5 * 60)).await;
                        State::Turning(Mode::Off)
                    }
                } else {
                    tokio::time::sleep(Duration::from_secs(5 * 60)).await;
                    State::Turning(mode)
                }
            }
        };
        if state != old_state {
            info!(message = "state change", old = ?old_state, new = ?state);
        }
    }
}

async fn handle(
    req: Request<Incoming>,
    tx: tokio::sync::broadcast::Sender<State>,
) -> Result<Response<Full<Bytes>>, Infallible> {
    let path = req.uri().path();

    match path {
        "/start_on" => {
            let _ = tx.send(State::Fading(Mode::On, 1.0));
            Ok(Response::new(Full::new(Bytes::from("Starting: ON"))))
        }
        "/start_off" => {
            let _ = tx.send(State::Fading(Mode::Off, 1.0));
            Ok(Response::new(Full::new(Bytes::from("Starting: OFF"))))
        }
        "/stop" => Ok(Response::new(Full::new(Bytes::from(
            "Stopped state machine",
        )))),
        "/" => {
            let html = include_str!("index.html");
            Ok(Response::new(Full::new(Bytes::from(html))))
        }
        _ => Ok(Response::builder()
            .status(404)
            .body(Full::new(Bytes::from("Not found")))
            .unwrap()),
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    init_tracing();
    let (tx, _rx) = broadcast::channel(8);
    let (rgb_tx, mut rgb_rx) = tokio::sync::watch::channel(0.0);

    tokio::spawn(async move {
        loop {
            match run_rgb_server(&mut rgb_rx).await {
                Ok(()) => break,
                Err(e) => error!("Error: {e:?}"),
            }
        }
    });

    let tx1 = tx.clone();
    tokio::spawn(async move {
        let tx = tx1.clone();
        let mut rx = tx.subscribe();
        let rgb_tx = rgb_tx.clone();
        let mut last_task = None;
        loop {
            let rgb_tx = rgb_tx.clone();
            if last_task.is_none() {
                last_task = Some(tokio::spawn(async {
                    run_state_machine(State::Turning(Mode::On), false, rgb_tx).await
                }));
                continue;
            }
            if let Ok(new_state) = rx.recv().await {
                if let Some(last_task) = last_task {
                    last_task.abort();
                }
                last_task = Some(tokio::spawn(async {
                    run_state_machine(new_state, true, rgb_tx).await
                }));
            }
        }
    });

    let addr = SocketAddr::from(([0, 0, 0, 0], 3000));
    let listener = TcpListener::bind(addr).await?;

    loop {
        let (stream, _) = listener.accept().await?;
        let io = TokioIo::new(stream);
        let tx = tx.clone();

        let service = service_fn(move |req| {
            let tx = tx.clone();
            async move { handle(req, tx).await }
        });

        tokio::spawn(async move {
            if let Err(err) = http1::Builder::new().serve_connection(io, service).await {
                error!("Error handling connection: {err:?}");
            }
        });
    }
}
