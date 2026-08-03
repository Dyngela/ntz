# Découpage et traits

Où sont les coutures, ce qu'elles ne doivent pas laisser fuir, et ce qu'il ne faut
**surtout pas** abstraire.

Complète [architecture.md](architecture.md) (le domaine) et [dependances.md](dependances.md)
(les dépendances concrètes).

---

## 1. Le principe : le crate fait la contrainte, pas le trait

Un `trait SqlSource` défini dans un crate qui dépend déjà de `sqlx` ne contient rien : rien
n'empêche d'importer `sqlx::PgPool` trois fichiers plus loin, et personne ne le remarquera
avant deux ans. **C'est la structure du workspace qui rend un trait réel.**

```
ntz/
  services/
    ntz-domain/    types purs        → arrow-schema, serde, chrono, thiserror
    ntz-ports/     les traits        → ntz-domain, arrow, futures, async-trait
    ntz-engine/    graphe → plans    → ntz-ports, DATAFUSION
    ntz-sql/       SqlSource/Sink    → ntz-ports, sqlx, arrow-odbc
    ntz-store/     persistance       → ntz-domain, sqlx  (concret, cf. §5)
    ntz-runtime/   process, Job Obj. → ntz-ports, windows, win32job
    ntz-notify/    mail, Teams       → ntz-ports, lettre, reqwest
    ntz-build/     builder Go        → ntz-ports, sha2, tempfile
    ntz-api/       HTTP              → tout le reste, axum, utoipa
    ntz-cli/       binaire `ntz`     → clap
  web/                               → React, à partir de l'étape 11
  xtask/                             → l'ordre de construction (architecture.md §9.2)
```

**La règle, et c'est la seule qui compte :** `ntz-domain` et `ntz-ports` ne dépendent
**jamais** de `datafusion`, `sqlx`, `arrow-odbc`, `axum` ni `windows`.

Et elle est **vérifiée mécaniquement**, pas seulement écrite ici. `deny.toml` bannit chaque
implémentation avec la liste des seuls crates autorisés à en dépendre directement :

```toml
[[bans.deny]]
name = "datafusion"
wrappers = ["ntz-engine"]     # tout autre dépendant fait échouer `cargo deny check`
```

Les entrées sont inertes tant que la dépendance n'est pas au graphe, et actives dès qu'elle
y entre. `tiberius` y figure sans aucun wrapper — interdit partout, pour qu'il ne revienne
pas par inadvertance (**D12**).

Deux autres règles de CLAUDE.md sont également rendues exécutables, dans
`[workspace.lints]` : `unsafe_code = "deny"` (levé dans le seul `ntz-runtime`, par un
`#![allow]` visible à côté du FFI) et `unwrap_used` / `expect_used` en `deny`, avec un
`#![cfg_attr(test, allow(...))]` par crate. Une convention que le compilateur n'applique pas
est une convention qu'on finit par oublier.

**`datafusion` n'apparaît que dans `ntz-engine`.** C'est ce qui rend une majeure toutes les
4 à 8 semaines (dependances.md §3.1) supportable : la casse d'API touche un crate, pas
douze.

---

## 2. Ce qui circule : Arrow, sans emballage

```rust
// ntz-ports
pub type BatchStream = futures::stream::BoxStream<'static, Result<RecordBatch, DataError>>;
```

`RecordBatch` et `SchemaRef` traversent toutes les couches **nus**. Ne pas les envelopper
dans un type ntz : Arrow *est* le format d'échange, stable et inter-langages. L'emballer
serait du coût pur, et rendrait `arrow-odbc` inutilisable tel quel — alors que son intérêt
principal est justement de produire des `RecordBatch` directement.

`BoxStream` vient de `futures`, pas de `SendableRecordBatchStream` de DataFusion :
même forme, mais ça garderait DataFusion dans les ports.

---

## 3. Les traits

Signatures seulement. Les types nommés ici (`Query`, `WriteOptions`, `Checkpoint`…) sont à
définir dans `ntz-domain` — c'est le premier travail de l'étape 5.

### 3.1 Bases de données

```rust
#[async_trait]
pub trait SqlSource: Send + Sync {
    /// Décrit le schéma SANS exécuter la requête (nodes.md §3, introspection).
    async fn describe(&self, q: &Query) -> Result<SchemaRef, SqlError>;

    /// Lit en flux. `resume_after` porte le point de reprise (architecture.md §5.3).
    async fn read(
        &self,
        q: &Query,
        batch_rows: usize,
        resume_after: Option<&Checkpoint>,
    ) -> Result<BatchStream, SqlError>;
}

#[async_trait]
pub trait SqlSink: Send + Sync {
    async fn write(
        &self,
        target: &TableRef,
        schema: SchemaRef,
        batches: BatchStream,
        opts: &WriteOptions,          // mode, key_columns, commit_mode, commit_every
        progress: &dyn ProgressSink,  // lignes écrites + checkpoint, au fil de l'eau
    ) -> Result<WriteOutcome, SqlError>;
}
```

**Ce que ce trait ne doit pas faire : niveler par le bas.** Il exprime une *intention*
(« écris ces lots dans cette table, en `Upsert`, par tranches de 50 000 ») et laisse chaque
implémentation prendre son chemin rapide — `COPY` binaire côté `sqlx`/PostgreSQL,
insertion en masse côté `arrow-odbc`. S'il exposait `execute_sql(&str)`, il aurait l'air
plus général et serait inutilisable : c'est exactement le piège du plus petit dénominateur
commun.

Les divergences de dialecte (`ON CONFLICT` contre table intermédiaire, échappement des
identifiants, `TRUNCATE`) vivent **dans les implémentations**, pas dans un `enum Dialect`
que tout le monde inspecte. Un `match dialect` qui remonte dans la logique métier est le
signe que la couture a échoué.

`ProgressSink` est ce qui fait sortir la progression sans que le sink connaisse ni la base
de métadonnées ni le SSE.

### 3.2 Le node, et la frontière plan / contrôle

Le point délicat. Un `Node::execute()` générique serait une erreur : il forcerait chaque
node à s'exécuter seul, ce qui interdit à DataFusion de fusionner une chaîne
`Scan → Filter → Project` en un seul plan, et donc de descendre les prédicats.

Le node décrit donc une **intention**, et `ntz-engine` la compile :

```rust
pub trait NodeKind: Send + Sync {
    fn kind(&self) -> &'static str;
    fn family(&self) -> Family;

    fn ports(&self, cfg: &NodeConfig) -> Result<Ports, ConfigError>;

    /// Propagation de schéma à la conception, sans rien exécuter.
    fn output_schemas(&self, cfg: &NodeConfig, inputs: &[SchemaRef])
        -> Result<Vec<SchemaRef>, SchemaError>;

    fn execution(&self, cfg: &NodeConfig) -> Execution;
}

/// LA frontière (architecture.md §2.2).
pub enum Execution {
    /// Opérateur relationnel : `ntz-engine` le fond dans un plan DataFusion.
    Relational(RelationalOp),
    /// Effet de bord : borne de plan, piloté par l'orchestrateur.
    SideEffect,
}

/// Vocabulaire relationnel de ntz — exprimé SANS DataFusion.
pub enum RelationalOp {
    Scan(ScanSpec),
    Project(Vec<ColumnMapping>),   // Map palier 1
    Filter(Predicate),
}
```

`Command`, `Script`, `HTTP` en `per_row` et tous les Output renvoient `SideEffect` : ils
deviennent des bornes, et c'est ce qui empêche DataFusion de les repartitionner ou de les
rejouer.

> **Le coût de la containment, dit franchement.** `RelationalOp` et `Predicate` sont un
> vocabulaire à écrire, qui recouvre partiellement `Expr` de DataFusion. Ce n'est pas
> gratuit.
>
> Le moins cher en v1 : **`Predicate` = un fragment SQL validé**, pas un AST. `ntz-engine`
> le parse avec l'analyseur SQL de DataFusion et en fait un `Expr`. Zéro AST à écrire, et
> ça colle à la décision « le langage exposé est du SQL » (**D9** → **D17**). N'écrire un
> AST que le jour où un constructeur visuel de prédicats l'exigera — donc pas en v1.

### 3.3 Le reste

```rust
#[async_trait]
pub trait ProcessRunner: Send + Sync {
    async fn run(&self, spec: &ProcessSpec, sink: &dyn LineSink) -> Result<ExitStatus, RunError>;
}

#[async_trait]
pub trait Builder: Send + Sync {
    fn language(&self) -> Language;
    async fn build(&self, source: &str) -> Result<Artifact, BuildError>;  // cache par source_hash
}

#[async_trait]
pub trait Notifier: Send + Sync {
    async fn notify(&self, event: &AlertEvent) -> Result<(), NotifyError>;
}

pub trait SecretStore: Send + Sync {
    fn seal(&self, plain: &[u8]) -> Result<Vec<u8>, CryptoError>;
    fn unseal(&self, sealed: &[u8]) -> Result<Zeroizing<Vec<u8>>, CryptoError>;
}

pub trait Clock: Send + Sync {
    fn now(&self) -> DateTime<Utc>;
}
```

`ProcessRunner` porte sa vraie valeur en test et hors Windows : une implémentation naïve
qui `spawn`, une implémentation Windows avec Job Objects (étape 22). C'est ce qui évite de
coder en dur des primitives Windows dans le scheduler.

`Clock` est trois lignes et rapporte immédiatement : c'est ce qui rend testable la
politique de changement d'heure (**D3**) sans jamais appeler `now()` dans un test —
exactement le critère d'acceptation de l'étape 2.

`SecretStore` rend `unseal` en `Zeroizing` pour que le secret déchiffré soit effacé à la
libération, et non laissé traîner en mémoire.

---

## 4. `async_trait` ou `async fn` natif ?

`async fn` en trait est stable depuis Rust 1.75, mais ne donne pas la compatibilité `dyn`.
Or on a besoin de `Box<dyn SqlSource>` : le type concret est choisi à l'exécution, selon la
`connection` configurée sur le node.

Donc **`async_trait` pour les traits appelés en `dyn`** (`SqlSource`, `SqlSink`,
`ProcessRunner`, `Notifier`, `Builder`), et `async fn` natif partout ailleurs. Ce n'est pas
un choix idéologique, c'est ce que le langage permet aujourd'hui.

---

## 5. Ce qu'il ne faut PAS abstraire

Autant de valeur que le reste du document. L'excès d'abstraction coûte plus cher que son
absence, parce qu'il est invisible.

| Couche | Pourquoi surtout pas |
|---|---|
| **Arrow** | C'est l'interface. L'envelopper casse l'intérêt d'`arrow-odbc` et de DataFusion. |
| **La base de métadonnées** | Un `trait JobRepository` pour « abstraire la base » est le piège classique. PostgreSQL est un engagement ferme, et `FOR UPDATE SKIP LOCKED`, le `JSONB`, les index partiels et les macros `sqlx` vérifiées à la compilation sont **porteurs**. Un trait générique les interdirait, ou les laisserait fuir — donc mentirait. `ntz-store` expose des fonctions concrètes, et se teste contre un vrai PostgreSQL en Docker. |
| **`axum`** | Une seule implémentation, aucune perspective d'en avoir deux. Abstraire un framework web est du coût pur. |
| **`serde` / JSON** | Lingua franca, comme Arrow. |
| **Le scheduler** | C'est le cœur métier, pas un détail technique remplaçable. Il n'a pas d'implémentation alternative. |

Le test à s'appliquer avant d'écrire un trait : **« quelle est la deuxième
implémentation ? »** Si la réponse est « un mock pour les tests », c'est souvent une
mauvaise raison — un vrai PostgreSQL en conteneur est plus fiable qu'un faux dépôt en
mémoire, et n'invente pas de sémantique. Si la réponse est « `arrow-odbc` en plus de
`sqlx` » ou « Job Objects en plus d'un `spawn` nu », le trait est justifié.

---

## 6. À lire

- [`TableProvider`](https://docs.rs/datafusion/latest/datafusion/catalog/trait.TableProvider.html)
  de DataFusion — c'est le modèle direct de notre `SqlSource`, et son mécanisme de descente
  de prédicats (`supports_filters_pushdown`) est exactement le problème à résoudre à
  l'étape 5.
- L'exemple [`custom_datasource`](https://github.com/apache/datafusion/blob/main/datafusion-examples/examples/custom_datasource.rs) —
  court, et à peu près ce que doit être un node source.
- La doc d'[`arrow-odbc`](https://docs.rs/arrow-odbc) sur `OdbcReader` et l'insertion en
  masse : à lire **avant** d'écrire `SqlSource`, pour que le trait épouse ce que la lib sait
  déjà faire plutôt que de le contrarier.
- Le trait [`Executor`](https://docs.rs/sqlx/latest/sqlx/trait.Executor.html) de `sqlx`,
  comme exemple d'abstraction de base réussie — et comme illustration de sa limite : il
  reste paramétré par la base, il ne prétend pas les unifier.
