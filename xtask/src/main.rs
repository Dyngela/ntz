//! Orchestration du build (architecture.md §9.2).
//!
//! Enchaînement à automatiser ici, qu'aucun de `cargo build` ni `vite build` ne
//! connaît seul :
//!
//!   cargo -> OpenAPI -> génération TS -> vite build -> rust-embed -> ntz.exe
//!
//! Ce crate n'est jamais livré : il ne sert qu'à la construction.
#![allow(clippy::print_stdout)]

fn main() {
    // Étape 12 — cf. doc/roadmap.md
}
