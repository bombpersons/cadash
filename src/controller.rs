//! Edit bindings here; game logic only consumes Mega Drive buttons.
use bevy::{input::keyboard::Key, prelude::*};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "dev-mcp", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "dev-mcp", serde(rename_all = "lowercase"))]
pub enum Button {
    Up,
    Down,
    Left,
    Right,
    A,
    B,
    C,
    Start,
}

impl Button {
    pub const ALL: [Self; 8] = [
        Self::Up,
        Self::Down,
        Self::Left,
        Self::Right,
        Self::A,
        Self::B,
        Self::C,
        Self::Start,
    ];
    pub fn mask(self) -> u16 {
        1 << self as u8
    }
}

/// Number of controller ports (Mega Drive pads 1 and 2).
pub const PORTS: usize = 2;

/// Both controller ports, sampled once per simulation tick.
#[derive(Resource, Default)]
pub struct Controllers {
    pub ports: [ControllerState; PORTS],
    /// A controller is plugged into the port (cartridge peripheral id 0xFF0208/0xFF0209 == 0x0D).
    /// Port 1 is always present; port 2 counts as present while its keyboard layout is enabled
    /// or a second gamepad is connected (see [`ControllerBindings::keyboard_p2_enabled`]).
    pub connected: [bool; PORTS],
}

impl Controllers {
    pub fn connected(&self, port: usize) -> bool {
        self.connected.get(port).copied().unwrap_or(false)
    }
}

#[derive(Default, Clone, Copy)]
pub struct ControllerState {
    held: u16,
    previous: u16,
}

impl ControllerState {
    pub fn update(&mut self, held: u16) {
        self.previous = self.held;
        self.held = held;
    }
    pub fn held(&self, button: Button) -> bool {
        self.held & button.mask() != 0
    }
    pub fn pressed(&self, button: Button) -> bool {
        (self.held & !self.previous) & button.mask() != 0
    }
    pub fn released(&self, button: Button) -> bool {
        (!self.held & self.previous) & button.mask() != 0
    }
    #[cfg(feature = "dev-mcp")]
    pub fn buttons(&self) -> Vec<Button> {
        Button::ALL.into_iter().filter(|b| self.held(*b)).collect()
    }
}

#[derive(Resource)]
pub struct ControllerBindings {
    /// Pad 1 keys (arrows, Z/X/C = A/B/C, Enter = Start).
    pub keyboard: [KeyCode; 8],
    /// Pad 2 keys (W/A/S/D, F/G/H = A/B/C, T = Start). Pad 2's gamepad is the second one connected.
    pub keyboard_p2: [KeyCode; 8],
    /// The pad-2 keyboard layout is plugged in (default). `--no-pad2` unplugs it, so pad 2 is then
    /// only present when a second gamepad is connected (2PLAYERS needs a pad in port 2).
    pub keyboard_p2_enabled: bool,
    /// The same keys by meaning (layout-aware). Some X setups (e.g. remote desktops) report
    /// arrow keys with the wrong physical code but the right logical key, so either matches.
    pub logical: [Key; 8],
    pub logical_p2: [Key; 8],
    pub gamepad: [GamepadButton; 8],
    pub stick_deadzone: f32,
}

impl Default for ControllerBindings {
    fn default() -> Self {
        Self {
            keyboard: [
                KeyCode::ArrowUp,
                KeyCode::ArrowDown,
                KeyCode::ArrowLeft,
                KeyCode::ArrowRight,
                KeyCode::KeyZ,
                KeyCode::KeyX,
                KeyCode::KeyC,
                KeyCode::Enter,
            ],
            keyboard_p2: [
                KeyCode::KeyW,
                KeyCode::KeyS,
                KeyCode::KeyA,
                KeyCode::KeyD,
                KeyCode::KeyF,
                KeyCode::KeyG,
                KeyCode::KeyH,
                KeyCode::KeyT,
            ],
            keyboard_p2_enabled: !std::env::args().any(|a| a == "--no-pad2"),
            logical: [
                Key::ArrowUp,
                Key::ArrowDown,
                Key::ArrowLeft,
                Key::ArrowRight,
                Key::Character("z".into()),
                Key::Character("x".into()),
                Key::Character("c".into()),
                Key::Enter,
            ],
            logical_p2: ["w", "s", "a", "d", "f", "g", "h", "t"].map(|c| Key::Character(c.into())),
            gamepad: [
                GamepadButton::DPadUp,
                GamepadButton::DPadDown,
                GamepadButton::DPadLeft,
                GamepadButton::DPadRight,
                GamepadButton::West,
                GamepadButton::South,
                GamepadButton::East,
                GamepadButton::Start,
            ],
            stick_deadzone: 0.25,
        }
    }
}

/// Keyboard state: physical key codes plus, when available, logical keys.
pub struct Keys<'a> {
    pub codes: &'a ButtonInput<KeyCode>,
    pub logical: Option<&'a ButtonInput<Key>>,
}

impl Keys<'_> {
    fn held(&self, code: KeyCode, key: &Key) -> bool {
        self.codes.pressed(code) || self.logical.is_some_and(|l| l.pressed(key.clone()))
    }
}

pub fn sample(
    keys: &ButtonInput<KeyCode>,
    pad: Option<&Gamepad>,
    bindings: &ControllerBindings,
) -> u16 {
    sample_port(0, &Keys { codes: keys, logical: None }, pad, bindings)
}

/// Buttons held on controller `port` (0 = pad 1, 1 = pad 2): its keyboard keys plus its gamepad.
pub fn sample_port(
    port: usize,
    keys: &Keys,
    pad: Option<&Gamepad>,
    bindings: &ControllerBindings,
) -> u16 {
    let keyboard = if port == 0 { &bindings.keyboard } else { &bindings.keyboard_p2 };
    let logical = if port == 0 { &bindings.logical } else { &bindings.logical_p2 };
    let use_keys = port == 0 || bindings.keyboard_p2_enabled;
    let mut bits = 0;
    for (index, button) in Button::ALL.into_iter().enumerate() {
        if (use_keys && keys.held(keyboard[index], &logical[index]))
            || pad.is_some_and(|pad| pad.pressed(bindings.gamepad[index]))
        {
            bits |= button.mask();
        }
    }
    if let Some(pad) = pad {
        let stick = pad.left_stick();
        if stick.x < -bindings.stick_deadzone {
            bits |= Button::Left.mask();
        }
        if stick.x > bindings.stick_deadzone {
            bits |= Button::Right.mask();
        }
        if stick.y < -bindings.stick_deadzone {
            bits |= Button::Down.mask();
        }
        if stick.y > bindings.stick_deadzone {
            bits |= Button::Up.mask();
        }
    }
    bits
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn edges_are_simulation_ticks_not_window_frames() {
        let mut pad = ControllerState::default();
        pad.update(Button::A.mask());
        assert!(pad.pressed(Button::A));
        assert!(pad.held(Button::A));
        pad.update(Button::A.mask());
        assert!(!pad.pressed(Button::A));
        pad.update(0);
        assert!(pad.released(Button::A));
        assert!(!pad.held(Button::A));
    }
    #[test]
    fn logical_arrow_with_wrong_physical_code_still_counts() {
        // Seen on an xrdp X session: ArrowRight arrives as physical NonConvert.
        let mut codes = ButtonInput::default();
        codes.press(KeyCode::NonConvert);
        let mut logical = ButtonInput::default();
        logical.press(Key::ArrowRight);
        let keys = Keys { codes: &codes, logical: Some(&logical) };
        assert_eq!(sample_port(0, &keys, None, &ControllerBindings::default()), Button::Right.mask());
    }
    #[test]
    fn default_keyboard_is_genesis_zxc_enter_and_arrows() {
        let mut keys = ButtonInput::default();
        keys.press(KeyCode::KeyX);
        keys.press(KeyCode::ArrowRight);
        assert_eq!(
            sample(&keys, None, &ControllerBindings::default()),
            Button::B.mask() | Button::Right.mask()
        );
    }
}
