# ntz

Plateforme de jobs planifiés du Groupe Kroely. Remplace une quarantaine
d'exécutables Windows lancés une fois par jour, sans historique consolidé, sans
logs conservés et sans alerte en cas d'échec.

Deux modes, un seul moteur :

- **`script`** — du code Go ou une commande existante, plus un planning. C'est le
  jalon **v0.5**, celui qui remplace Task Scheduler.
- **`graph`** — un DAG de nodes entre lesquels circule de la donnée : un ETL
  visuel, avec éditeur de nodes.

Rust · PostgreSQL · Arrow + DataFusion · React · Windows Server.

---

## Où lire quoi

**Commencer par [CLAUDE.md](CLAUDE.md)** : les invariants du projet, les bonnes
pratiques, et ce qu'il ne faut pas re-proposer.

| Document | Contenu |
|---|---|
| [doc/roadmap.md](doc/roadmap.md) | 24 étapes, jalon v0.5, critères d'acceptation |
| [doc/architecture.md](doc/architecture.md) | modèle de données, schéma SQL, sémantique d'exécution, les 19 décisions |
| [doc/nodes.md](doc/nodes.md) | spécification node par node |
| [doc/traits.md](doc/traits.md) | découpage en crates, traits, et ce qu'il ne faut pas abstraire |
| [doc/dependances.md](doc/dependances.md) | maintenance, coûts, alternatives (daté) |
| [doc/react-depuis-angular.md](doc/react-depuis-angular.md) | transition Angular → React |

## Structure

```
services/     les crates Rust (voir doc/traits.md §1)
web/          le front React — à partir de l'étape 11
xtask/        orchestration du build (architecture.md §9.2)
doc/          la conception
deny.toml     licences, avis de sécurité, et la règle d'architecture rendue exécutable
```

## Démarrer

```sh
cargo build
cargo test
cargo fmt --all --check
cargo clippy --all-targets -- -D warnings
cargo deny check          # nécessite `cargo install cargo-deny`
```

La toolchain est épinglée dans `rust-toolchain.toml` : `rustup` installe la bonne
version automatiquement au premier `cargo`.

## État

Étape 1 en cours — squelette du workspace en place, `ntz run <commande>` à écrire.
