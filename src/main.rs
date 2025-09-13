use anyhow::Result;
use chrono::{Local, Timelike};
use hyper::service::{make_service_fn, service_fn};
use hyper::{Body, Server};
use hyper::{Request, Response};
use openrgb2::{Color, DeviceType, OpenRgbClient};
use std::convert::Infallible;
use std::env;
use std::time::Duration;
use tokio::sync::broadcast;
use tracing::{Subscriber, info};
use tracing_journald;
use tracing_subscriber::{EnvFilter, Registry, prelude::*};

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

async fn run_state_machine(starting_state: State, mut long_wait_when_idle: bool) -> Result<()> {
    let client = OpenRgbClient::connect().await?;
    let mobo = client
        .get_controllers_of_type(DeviceType::Motherboard)
        .await?;
    let dram = client.get_controllers_of_type(DeviceType::DRam).await?;
    let mut state = starting_state;
    loop {
        let old_state = state.clone();
        state = match state {
            State::SetColourThen(x, new_state) => {
                for controller in mobo.controllers() {
                    controller
                        .set_all_leds(Color::new(
                            (54.0 * x) as u8,
                            (28.0 * x) as u8,
                            (15.0 * x) as u8,
                        ))
                        .await?;
                }

                for controller in dram.controllers() {
                    controller
                        .set_all_leds(Color::new(
                            (89.0 * x) as u8,
                            (60.0 * x) as u8,
                            (46.0 * x) as u8,
                        ))
                        .await?;
                }

                *new_state
            }
            State::Fading(mode, raw_x) => {
                let x = match mode {
                    Mode::On => 1.0 - raw_x,
                    Mode::Off => raw_x,
                };

                if raw_x > 0.08 {
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    State::SetColourThen(x * 0.5, Box::new(State::Fading(mode, raw_x * 0.5)))
                } else {
                    State::Turning(mode)
                }
            }
            State::Turning(mode) => match mode {
                Mode::On => State::SetColourThen(1.0, Box::new(State::Idle(mode))),
                Mode::Off => State::SetColourThen(0.0, Box::new(State::Idle(mode))),
            },
            State::Idle(mode) => {
                if !long_wait_when_idle {
                    tokio::time::sleep(Duration::from_secs(15)).await;
                } else {
                    tokio::time::sleep(Duration::from_secs(60 * 60 * 3)).await;
                }
                long_wait_when_idle = false;
                let hour = Local::now().hour();
                if hour >= 8 && hour < 18 {
                    if mode == Mode::Off {
                        State::Fading(Mode::On, 1.0)
                    } else {
                        tokio::time::sleep(Duration::from_secs(60)).await;
                        State::Turning(Mode::On)
                    }
                } else if hour >= 18 {
                    if mode == Mode::On {
                        State::Fading(Mode::Off, 1.0)
                    } else {
                        tokio::time::sleep(Duration::from_secs(60)).await;
                        State::Turning(Mode::Off)
                    }
                } else {
                    tokio::time::sleep(Duration::from_secs(60)).await;
                    State::Turning(mode)
                }
            }
        };
        if state != old_state {
            info!("state change: old={:?}, new={:?}", old_state, state);
        }
    }
}

async fn handle(
    req: Request<Body>,
    tx: tokio::sync::broadcast::Sender<State>,
) -> Result<Response<Body>, Infallible> {
    let path = req.uri().path();

    match path {
        "/start_on" => {
            let _ = tx.send(State::Fading(Mode::On, 1.0));
            Ok(Response::new(Body::from("Starting: ON")))
        }
        "/start_off" => {
            let _ = tx.send(State::Fading(Mode::Off, 1.0));
            Ok(Response::new(Body::from("Starting: OFF")))
        }
        "/stop" => Ok(Response::new(Body::from("Stopped state machine"))),
        "/" => {
            let html = include_str!("index.html");
            Ok(Response::new(Body::from(html)))
        }
        _ => Ok(Response::builder()
            .status(404)
            .body(Body::from("Not found"))
            .unwrap()),
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    init_tracing();
    let (tx, _rx) = broadcast::channel(8);
    let tx1 = tx.clone();
    let make_svc = make_service_fn(move |_| {
        let tx = tx.clone();
        async move { Ok::<_, Infallible>(service_fn(move |req| handle(req, tx.clone()))) }
    });

    tokio::spawn(async move {
        let tx = tx1.clone();
        let mut rx = tx.subscribe();
        let mut last_task = None;
        loop {
            if let None = last_task {
                last_task = Some(tokio::spawn(async {
                    run_state_machine(State::Turning(Mode::On), false).await
                }));
                continue;
            }
            if let Ok(new_state) = rx.recv().await {
                if let Some(last_task) = last_task {
                    last_task.abort();
                }
                last_task = Some(tokio::spawn(async {
                    run_state_machine(new_state, true).await
                }));
            }
        }
        //rx.try_recv()
    });

    let addr = ([0, 0, 0, 0], 3000).into();
    println!("Listening on http://{}", addr);
    if let Err(e) = Server::bind(&addr).serve(make_svc).await {
        eprintln!("server error: {}", e);
    }
    Ok(())
}
