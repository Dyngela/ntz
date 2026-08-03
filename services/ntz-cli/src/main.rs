//! Binaire `ntz`.
//!
//! Le CLI est le seul endroit du workspace où stdout est l'interface : le lint
//! `print_stdout` du workspace y est donc levé. Ailleurs, la sortie passe par
//! `tracing`.
#![allow(clippy::print_stdout)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

fn main() {
    // Étape 1 — `ntz run <commande>` : cf. doc/roadmap.md
}
