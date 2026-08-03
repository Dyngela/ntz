//! Exécution de process : spawn, arbre de process, Windows Job Objects.

//! Seul crate du workspace autorisé à écrire du `unsafe` (FFI Windows), et
//! uniquement encapsulé derrière un type RAII.
#![allow(unsafe_code)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]
