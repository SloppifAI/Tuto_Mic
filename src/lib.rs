//! Tuto Mic: Unterricht aufnehmen, lokal transkribieren, pseudonymisieren und zusammenfassen.
//!
//! Die Logik liegt in dieser Bibliothek, damit sie ohne GUI testbar ist; `main.rs` ist nur der Einstieg
//! (Kommandozeile bzw. Fenster).

pub mod anonymize;
pub mod app;
pub mod audio;
pub mod config;
pub mod control;
pub mod deeplink;
pub mod gpu;
pub mod icons;
pub mod llm;
pub mod models;
pub mod pipeline;
pub mod prompts;
pub mod recorder;
pub mod session;
pub mod theme;
pub mod transcribe;
pub mod tuto;
pub mod tuto_panel;

#[cfg(test)]
mod test_http;
