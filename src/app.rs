use std::sync::{Arc, Mutex};

use color_eyre::eyre::Result;
use crossterm::event::KeyEvent;
use ratatui::{
  layout::{Constraint, Direction, Layout},
  prelude::Rect,
  widgets::{Block, Borders, Paragraph},
};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

use crate::{
  action::Action,
  components::{data::Data, ide::IDE, menu::Menu, Component},
  config::Config,
  focus::Focus,
  tui,
};

pub struct AppState {
  pub connection_string: String,
  pub focus: Focus,
}

pub struct Components {
  pub menu: Box<dyn Component>,
  pub ide: Box<dyn Component>,
  pub data: Box<dyn Component>,
}

impl Components {
  pub fn to_array(&mut self) -> [&mut Box<dyn Component>; 3] {
    [&mut self.menu, &mut self.ide, &mut self.data]
  }
}

pub struct App {
  pub config: Config,
  pub tick_rate: Option<f64>,
  pub frame_rate: Option<f64>,
  pub components: Components,
  pub should_quit: bool,
  pub last_tick_key_events: Vec<KeyEvent>,
  pub state: Arc<Mutex<AppState>>,
}

impl App {
  pub fn new(connection_string: String, tick_rate: Option<f64>, frame_rate: Option<f64>) -> Result<Self> {
    let focus = Focus::Menu;
    let state = Arc::new(Mutex::new(AppState { connection_string, focus }));
    let menu = Menu::new(Arc::clone(&state));
    let ide = IDE::new(Arc::clone(&state));
    let data = Data::new(Arc::clone(&state));
    let config = Config::new()?;
    Ok(Self {
      state: Arc::clone(&state),
      tick_rate,
      frame_rate,
      components: Components { menu: Box::new(menu), ide: Box::new(ide), data: Box::new(data) },
      should_quit: false,
      config,
      last_tick_key_events: Vec::new(),
    })
  }

  pub async fn run(&mut self) -> Result<()> {
    let (action_tx, mut action_rx) = mpsc::unbounded_channel();

    let mut tui = tui::Tui::new()?.tick_rate(self.tick_rate).frame_rate(self.frame_rate);
    // tui.mouse(true);
    tui.enter()?;

    for component in self.components.to_array().iter_mut() {
      component.register_action_handler(action_tx.clone())?;
    }

    for component in self.components.to_array().iter_mut() {
      component.register_config_handler(self.config.clone())?;
    }

    for component in self.components.to_array().iter_mut() {
      component.init(tui.size()?)?;
    }

    loop {
      if let Some(e) = tui.next().await {
        match e {
          tui::Event::Quit => action_tx.send(Action::Quit)?,
          tui::Event::Tick => action_tx.send(Action::Tick)?,
          tui::Event::Render => action_tx.send(Action::Render)?,
          tui::Event::Resize(x, y) => action_tx.send(Action::Resize(x, y))?,
          tui::Event::Key(key) => {
            if let Some(keymap) = self.config.keybindings.get(&self.state.lock().unwrap().focus) {
              if let Some(action) = keymap.get(&vec![key]) {
                log::info!("Got action: {action:?}");
                action_tx.send(action.clone())?;
              } else {
                // If the key was not handled as a single key action,
                // then consider it for multi-key combinations.
                self.last_tick_key_events.push(key);

                // Check for multi-key combinations
                if let Some(action) = keymap.get(&self.last_tick_key_events) {
                  log::info!("Got action: {action:?}");
                  action_tx.send(action.clone())?;
                }
              }
            };
          },
          _ => {},
        }
        for component in self.components.to_array().iter_mut() {
          if let Some(action) = component.handle_events(Some(e.clone()))? {
            action_tx.send(action)?;
          }
        }
      }

      while let Ok(action) = action_rx.try_recv() {
        if action != Action::Tick && action != Action::Render {
          log::debug!("{action:?}");
        }
        match action {
          Action::Tick => {
            self.last_tick_key_events.drain(..);
          },
          Action::Quit => self.should_quit = true,
          Action::Resize(w, h) => {
            tui.resize(Rect::new(0, 0, w, h))?;
            tui.draw(|f| {
              for component in self.components.to_array().iter_mut() {
                let r = component.draw(f, f.size());
                if let Err(e) = r {
                  action_tx.send(Action::Error(format!("Failed to draw: {:?}", e))).unwrap();
                }
              }
            })?;
          },
          Action::Render => {
            tui.draw(|f| {
              let root_layout = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Percentage(25), Constraint::Percentage(75)])
                .split(f.size());
              let right_layout = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
                .split(root_layout[1]);

              self.components.menu.draw(f, root_layout[0]).unwrap();
              self.components.ide.draw(f, right_layout[0]).unwrap();
              self.components.data.draw(f, right_layout[1]).unwrap();
            })?;
          },
          Action::FocusMenu => {
            let mut state = self.state.lock().unwrap();
            state.focus = Focus::Menu;
          },
          Action::FocusIDE => {
            let mut state = self.state.lock().unwrap();
            state.focus = Focus::IDE;
          },
          Action::FocusData => {
            let mut state = self.state.lock().unwrap();
            state.focus = Focus::Data;
          },
          _ => {},
        }
        for component in self.components.to_array().iter_mut() {
          if let Some(action) = component.update(action.clone())? {
            action_tx.send(action)?
          };
        }
      }
      if self.should_quit {
        tui.stop()?;
        break;
      }
    }
    tui.exit()?;
    Ok(())
  }
}