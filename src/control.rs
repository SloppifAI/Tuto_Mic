//! Befehle, die die Aufnahme steuern. Die GUI ist aktuell die einzige Quelle; ein externer
//! Hardware-Taster (z. B. Mikrocontroller per USB-Seriell) soll später dieselben Befehle senden
//! und den [`RecState`] als LED anzeigen.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    Start,
    Pause,
    Resume,
    Stop,
    /// Die letzten N Minuten der laufenden Aufnahme verwerfen.
    DeleteLast {
        minutes: u32,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecState {
    Idle,
    Recording,
    Paused,
    Processing,
}
