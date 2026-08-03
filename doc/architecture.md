# ntz — Architecture

Document de référence sur le modèle de données et la sémantique d'exécution.
La [roadmap](roadmap.md) dit *quand* construire quoi, [nodes.md](nodes.md) spécifie
chaque node, ce document dit *quoi* et *pourquoi*.

Runtime cible v1 : **Go** (node `Script`). Plateforme v1 : **Windows Server**.
Base de métadonnées : **PostgreSQL**. Bases cibles des jobs : **PostgreSQL et SQL Server**.

**Deux hypothèses posées faute d'arbitrage, révisables :**

- **H1** — « HTTP Input » est lu comme *aller chercher de la donnée*, pas comme un
  webhook. Un seul node `HTTP Request`. Le déclenchement par webhook est un
  **déclencheur**, pas un node : voir **D13**.
- **H2** — les schémas de port sont **déclarés** (modèle Talend) et non dynamiques
  (modèle n8n), mais remplis par **introspection** plutôt qu'à la main. Sans schéma
  déclaré, le node `Map` et la validation de connexion sont infaisables.

---

## 1. Le modèle : un job est un graphe de données

Un job n'est pas une commande, c'est un **graphe de nodes entre lesquels circule de la
donnée**. Les arêtes transportent des jeux de lignes typées.

```
[Start] ──▶ [SQL Input] ──▶ [Filter] ──▶ [Map] ──▶ [SQL Output] ──▶ [End]
                                          │
                                     (rejets) ──▶ [File Output]
```

C'est un **ETL visuel**, pas un orchestrateur de crons. La différence n'est pas
cosmétique : elle détermine le moteur d'exécution, le modèle de reprise sur incident,
et la moitié du travail d'interface.

### Vocabulaire

| Terme | Définition |
|---|---|
| **Job** | Unité nommée et versionnée. Peut être invoqué comme un node dans un autre job. |
| **Draft** | Graphe **mutable**, un seul par job. Ce que l'éditeur manipule. Peut être invalide. |
| **Version** | Snapshot **immuable** d'un draft, publié et validé. Seules les versions s'exécutent. |
| **Node** | Une étape. Un `kind`, une `config`, des ports d'entrée et de sortie. |
| **Port** | Point de branchement typé. Porte un schéma. |
| **Edge** | Arête port-à-port. De **données** ou de **contrôle**. |
| **Dataset** | Ce qui circule sur une arête de données : un schéma + des lignes. |
| **Schedule** | Une expression cron + un fuseau, rattachée à un job. |
| **Run** | Une exécution d'une version. **L'unité de réclamation et de retry.** |
| **NodeRun** | Trace d'exécution d'un node dans un run. Observabilité, pas ordonnancement. |

### 1.1 Deux modes de rédaction, un seul moteur

Tous les jobs ne méritent pas un graphe. `job.kind` en distingue deux :

| Mode | Ce que voit l'utilisateur | Graphe réel |
|---|---|---|
| `graph` | le canvas de nodes | celui qu'il a dessiné |
| `script` | un éditeur de code ou une commande, et un planning | `Start → Command`\|`Script → End`, **synthétisé à la publication** |

Le mode `script` **ne double pas le moteur**. Son draft ne contient que
`{ language, source | command, params }`, et la publication en dérive le graphe canonique
à trois nodes. Un second chemin d'exécution coûterait deux schedulers, deux modèles de
retry, deux pipelines de logs et deux télémétries — pour un graphe linéaire dont le coût
d'exécution est nul.

Trois conséquences, toutes bonnes :

- un job `script` est utilisable comme node `Job` dans un job `graph`, sans travail
  supplémentaire ;
- historique, logs, SSE, alerting et métriques sont rigoureusement identiques dans les
  deux modes ;
- un job `script` peut être **converti** en `graph` — on débloque le canvas sur son
  graphe synthétisé. L'inverse n'est pas proposé.

C'est surtout le mode qui porte la valeur à court terme : remplacer Task Scheduler et les
exe historiques ne demande ni node SQL, ni canvas, ni `Map`. Il n'a besoin que de la
persistance, de l'API et du node `Command` — d'où le jalon **v0.5** de la roadmap, bien
avant que le mode `graph` soit complet.

### Draft / version : pourquoi la séparation

- **Déplacer un node à la souris ne doit pas créer une version.** Les coordonnées
  `(x, y)` sont de l'affichage. Elles n'entrent pas dans `graph_hash`.
- **L'éditeur a besoin d'un état invalide.** Pendant qu'on câble, un port est non
  branché, un schéma est incompatible. Un draft peut être invalide, une version jamais.

La validation complète a lieu **à la publication**. Le draft s'auto-sauvegarde
librement, et l'API sait à la demande dire ce qui ne va pas (§7).

---

## 2. Données : représentation et volumétrie

### 2.1 La contrainte dimensionnante

Volumétrie cible communiquée : **jusqu'à 900 millions de lignes de 350 champs**, soit
**≈ 315 milliards de cellules** et, à 20 octets de moyenne par valeur, **de l'ordre de
6 To de données brutes**.

Ce n'est pas une volumétrie faible. C'est l'ordre de grandeur d'une grosse table
d'entrepôt de données, et c'est **la contrainte qui dimensionne toute l'architecture**.
Elle disqualifie la conception ligne-à-ligne matérialisée qui figurait ici auparavant.

Le calcul, avec une représentation orientée ligne (`Row(Vec<Value>)`, un enum `Value`
par cellule) :

| | Calcul | Résultat |
|---|---|---|
| Taille d'un `Value` | discriminant + plus grosse variante | ≥ 32 octets |
| Ossature d'une ligne | 350 × 32 + 24 | ≈ **11,2 Ko** hors chaînes |
| Allocations par ligne | une par champ texte non vide | ~10² |
| Plafond sur 16 Go de RAM | 16 Go / 11,2 Ko | ≈ **1,4 million de lignes** |
| Les 900 M de lignes en mémoire | 315 × 10⁹ × 32 octets | ≈ **10 To** d'ossature seule |

Le plafond n'est pas 900 millions de lignes, c'est **1,4 million** — un facteur 640. Et
900 millions de lignes représenteraient de l'ordre de 10¹¹ allocations mémoire.

Deux conséquences non négociables :

1. **Le flux est le mode par défaut, pas une optimisation ultérieure.** C'est
   l'inversion de **D10**.
2. **La représentation est en colonnes, pas en lignes.** Un enum tagué par cellule est
   inabordable à cette échelle : le type doit être porté par la colonne, une seule fois.

### 2.2 Représentation : Arrow, en colonnes, par lots

Ce qui circule sur une arête de données est un **flux de lots colonnaires**
[Apache Arrow](https://docs.rs/arrow) :

```rust
/// Ce que transporte une arête `data`.
pub type Dataset = BoxStream<'static, Result<RecordBatch, DataError>>;
```

Un `RecordBatch` est un `Schema` plus un tableau par colonne. Ce que ça change :

| | Orienté ligne (abandonné) | Arrow colonnaire (retenu) |
|---|---|---|
| Type | un tag par cellule (315 × 10⁹ tags) | un `DataType` par colonne (350 au total) |
| Chaînes | une allocation par cellule | un tampon contigu + tableau d'offsets par lot |
| Nullité | une variante `Null` par cellule | un bitmap de validité, 1 bit par cellule |
| Mémoire de pointe | proportionnelle au volume **total** | `taille_de_lot × profondeur_du_pipeline` |
| Parcours d'une colonne | saut de 11 Ko entre deux valeurs | contigu, vectorisable |

**La mémoire de pointe devient indépendante du volume total.** C'est tout l'enjeu : avec
des lots de 1024 lignes × 350 colonnes ≈ 7 Mo, un pipeline de six nodes tient dans
quelques centaines de mégaoctets, que la source fasse un million ou neuf cents millions
de lignes.

Arrow apporte aussi le système de types que j'avais écrit à la main ici — en moins bien.
`DataType::Decimal128 { precision, scale }`, `Date32`, `Timestamp(TimeUnit, tz)`,
`Utf8`/`LargeUtf8` : c'est le même besoin, déjà résolu, déjà testé, et interopérable.

Un point conservé de la version précédente, et il compte : **`Decimal128`, jamais
`Float64`, pour tout montant.** La donnée d'un groupe de concessions est en grande
partie financière ; `0.1 + 0.2 != 0.3` en binaire, et une erreur d'arrondi sur une marge
n'est pas un bug qu'on découvre vite.

#### DataFusion assure le plan de données

[DataFusion](https://datafusion.apache.org) est le moteur d'exécution retenu (**D17**).
Ce qu'il apporte est exactement la liste de ce que ce document repoussait à « plus tard » :

| Besoin | Sans DataFusion | Avec |
|---|---|---|
| Exécution en flux avec contre-pression | à écrire | `ExecutionPlan`, `SendableRecordBatchStream` |
| Déversement sur disque (tri, jointure, agrégation) | « une étape en soi » | `MemoryPool`, `DiskManager` |
| Parallélisme, repartitionnement | à écrire | intégré |
| Descente de prédicats et de projections | à écrire | intégré |
| Évaluation d'expressions **vectorisée** | Rhai par ligne (**D9**) | `Expr` sur colonnes Arrow |

La descente de prédicats n'est pas un détail à cette échelle : un `SQL Input → Filter`
dont le filtre remonte dans la requête source, c'est la différence entre déplacer 6 To
et en déplacer 50 Go.

#### La frontière, et c'est le point à ne pas manquer

**DataFusion remplace le moteur de dataflow, pas l'orchestrateur.** Un plan de requête
n'a aucune notion de « exécute cette branche si l'autre a échoué ». Restent donc
intégralement à la charge de ntz :

- les arêtes de **contrôle**, `on_success` / `on_failure` / `always`, `join_policy` (§3, §5.2) ;
- le retry, les baux, les **points de reprise** (§5.3) — DataFusion ne reprend pas un plan
  en cours de route ;
- l'ordonnancement des runs, l'idempotence des créneaux, la persistance.

**Corollaire : le graphe ne compile pas en un plan, mais en plusieurs, découpés aux
frontières d'effet de bord.** Un plan de requête suppose ses opérateurs purs et
rejouables — il peut les repartitionner ou les réexécuter. Y glisser « appelle cette API
pour chaque ligne » ou « lance ce process » est un piège. Donc les nodes à effet de bord
(`HTTP Request` en `per_row`, `Script`, `Command`, et les Output) sont des **bornes de
plan**, pas des opérateurs dans un plan :

```
[SQL Input]─[Filter]─[Map] │ [HTTP per_row] │ [Map]─[SQL Output]
└──── plan DataFusion ─────┘  ntz, hors plan  └── plan DataFusion ──┘
```

C'est l'orchestrateur ntz qui découpe, enchaîne les tronçons et gère ce qui se passe
entre eux. Cette frontière est la décision d'architecture la plus structurante du
document après §5.1 — s'y tromper, c'est soit réécrire DataFusion, soit lui confier une
sémantique qu'il ne garantit pas.

### 2.3 Nodes bloquants et nodes passants

Chaque node déclare son comportement, et cette fois la distinction **coûte** :

| | Définition | Exemples | Mémoire |
|---|---|---|---|
| **Passant** | traite lot par lot, émet au fil de l'eau | `Filter`, `Map` palier 1, `HTTP per_row`, tous les Output | bornée |
| **Bloquant** | a besoin de tout le flux avant d'émettre | tri, agrégation, jointure, `Script` | **proportionnelle au volume** |

Deux régimes, et la distinction change de nature selon qui exécute :

- **dans un plan DataFusion**, un opérateur bloquant (tri, jointure, agrégation) est
  acceptable : le `MemoryPool` et le `DiskManager` déversent sur disque au-delà d'un seuil
  configuré. C'est ce qui débloque le palier 3 de `Map` (jointures, lookups) bien plus tôt
  que prévu — il n'y a plus de moteur hors-mémoire à écrire ;
- **hors plan**, un node à effet de bord qui accumule tout avant d'émettre ramène le
  problème de §2.1 sans filet. `Script` est le cas à surveiller : rien n'empêche le code
  de l'utilisateur de tout garder en mémoire.

D'où deux règles qui restent :

- **plafonner explicitement le `MemoryPool`** plutôt que de laisser l'OOM arbitrer — un
  déversement lent et visible vaut mieux qu'un processus tué ;
- l'éditeur **signale visuellement** un node bloquant hors plan, avec l'avertissement
  associé.

### 2.4 Ce que la volumétrie impose ailleurs

Cette contrainte ne reste pas confinée à §2. Elle se propage :

| Où | Conséquence |
|---|---|
| §5.3 Reprise | Un job de 900 M de lignes dure des heures. Le rejouer intégralement après un échec à 95 % est inacceptable : il faut des **points de reprise**. |
| §5.4 Transactions | Une transaction unique sur 900 M d'insertions fait exploser le WAL PostgreSQL ou le journal SQL Server. **Commits par tranches**, avec la perte d'atomicité que ça implique (**D18**). |
| [nodes.md](nodes.md) SQL Output | Les limites de paramètres (2100 en SQL Server, 65535 en PostgreSQL) plafonnent un `INSERT` paramétré à **6 lignes** par ordre à 350 colonnes. `COPY` et le bulk load TDS ne sont pas des optimisations, ce sont les seules voies praticables. |
| [nodes.md](nodes.md) `Map` | Câbler 350 champs à la souris n'est pas une interface. L'**auto-mapping par nom** devient le comportement par défaut, l'éditeur ne sert qu'aux exceptions. |
| Inspection des données | On n'inspecte pas un dataset, on inspecte **les N premières lignes d'un lot**. La fonctionnalité reste, sa promesse change. |
| Débit | 6 To à déplacer : à 500 Mo/s soutenus, ≈ 3 h 30 de pur transfert. Une **barre de progression et un débit instantané** ne sont pas cosmétiques. |

---

## 3. Ports et arêtes

```rust
pub struct PortSpec {
    pub key: String,
    pub label: String,
    pub kind: PortKind,
    pub required: bool,
}

pub enum PortKind {
    Data,      // transporte un Dataset
    Error,     // transporte les lignes rejetées + la cause
    Control,   // ne transporte rien : signale un enchaînement
}
```

Une arête relie **un port à un port**, pas un node à un node :

| Type d'arête | Transporte | Conditionnée | Usage |
|---|---|---|---|
| `data` | un `Dataset` | non | le flux normal, et les rejets |
| `control` | rien | `on_success` / `on_failure` / `always` | enchaîner, brancher, rattraper une erreur |

Les deux modèles coexistent, et c'est nécessaire : `Condition` branche un **flux**
(contrôle), `Filter` trie des **lignes** (données). Ce sont deux besoins différents que
le document initial confondait sous un seul node.

**Règles de branchement**, vérifiées à la publication et dans l'éditeur :

- un port d'entrée `Data` accepte **au plus une** arête entrante (pas de fusion
  implicite ; fusionner est le travail explicite d'un node `Union`, hors v1) ;
- un port de sortie `Data` accepte **N** arêtes sortantes — le dataset est partagé, et
  comme les lignes sont immuables, il est partagé par `Arc`, pas cloné ;
- les schémas doivent être **compatibles** : mêmes colonnes, et types convertibles sans
  perte. Élargir (`Int32` → `Int64`) passe, rétrécir non.

### Gestion d'erreur ligne à ligne

Absente du document initial, et indispensable : sur cent mille lignes dont trois sont
invalides, que fait-on ? Trois politiques par node (`node.error_policy`) :

| Politique | Comportement |
|---|---|
| `die` (défaut) | la première ligne en erreur fait échouer le node |
| `ignore` | la ligne est écartée, comptée, journalisée ; le node continue |
| `reject` | la ligne part sur le port `Error` avec sa cause, et devient un flux comme un autre |

`reject` est ce qui rend le système utilisable en production : un import de stock qui
tombe sur trois références inconnues doit charger les 99 997 autres et déposer les
trois dans un CSV que quelqu'un ira regarder.

---

## 4. Schéma de base

`sqlx` avec migrations. Les énumérés sont des `TEXT` + `CHECK` plutôt que des `ENUM`
PostgreSQL : ajouter une valeur à un `ENUM` est pénible en migration, et `sqlx` mappe
très bien un `TEXT` sur un enum Rust.

```sql
-- ── Définition ────────────────────────────────────────────────────────────────

CREATE TABLE job (
    id           BIGSERIAL PRIMARY KEY,
    name         TEXT NOT NULL UNIQUE,
    description  TEXT,
    -- 'graph'  : l'utilisateur dessine le graphe
    -- 'script' : du code ou une commande ; le graphe est synthétisé (§1.1)
    kind         TEXT NOT NULL DEFAULT 'graph' CHECK (kind IN ('graph', 'script')),
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    archived_at  TIMESTAMPTZ
);

-- Un draft mutable par job. Le graphe entier en JSONB : il n'est jamais requêté
-- par morceaux, seulement lu et réécrit en bloc par l'éditeur.
CREATE TABLE job_draft (
    job_id     BIGINT PRIMARY KEY REFERENCES job(id) ON DELETE CASCADE,
    graph      JSONB NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_by TEXT NOT NULL
);

CREATE TABLE job_version (
    id           BIGSERIAL PRIMARY KEY,
    job_id       BIGINT NOT NULL REFERENCES job(id),
    version_no   INT NOT NULL,
    graph_hash   TEXT NOT NULL,        -- sha256 du graphe normalisé, hors positions
    signature    JSONB NOT NULL,       -- params d'entrée/sortie : le contrat du job
    published_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    published_by TEXT NOT NULL,
    UNIQUE (job_id, version_no)
);

CREATE TABLE node (
    id              BIGSERIAL PRIMARY KEY,
    job_version_id  BIGINT NOT NULL REFERENCES job_version(id) ON DELETE CASCADE,
    key             TEXT NOT NULL,     -- stable, lisible, choisi par l'utilisateur
    kind            TEXT NOT NULL,
    config          JSONB NOT NULL,    -- validé contre le JSON Schema du kind
    error_policy    TEXT NOT NULL DEFAULT 'die'
                        CHECK (error_policy IN ('die', 'ignore', 'reject')),
    join_policy     TEXT NOT NULL DEFAULT 'all'
                        CHECK (join_policy IN ('all', 'any')),
    timeout_seconds INT,
    position        JSONB NOT NULL,    -- {"x": .., "y": ..} — hors graph_hash
    UNIQUE (job_version_id, key)
);

CREATE TABLE edge (
    id             BIGSERIAL PRIMARY KEY,
    job_version_id BIGINT NOT NULL REFERENCES job_version(id) ON DELETE CASCADE,
    kind           TEXT NOT NULL CHECK (kind IN ('data', 'control')),
    from_node_id   BIGINT NOT NULL REFERENCES node(id) ON DELETE CASCADE,
    from_port      TEXT NOT NULL,
    to_node_id     BIGINT NOT NULL REFERENCES node(id) ON DELETE CASCADE,
    to_port        TEXT NOT NULL,
    condition      TEXT CHECK (condition IN ('on_success', 'on_failure', 'always')),
    UNIQUE (job_version_id, from_node_id, from_port, to_node_id, to_port),
    -- une arête de données n'est pas conditionnée, une arête de contrôle l'est toujours
    CHECK ((kind = 'data' AND condition IS NULL)
        OR (kind = 'control' AND condition IS NOT NULL))
);

-- Un port d'entrée de données n'accepte qu'une seule arête.
CREATE UNIQUE INDEX edge_single_data_input_idx
    ON edge (job_version_id, to_node_id, to_port) WHERE kind = 'data';

-- Qui embarque qui, pour la vérification de contrat à la publication (§7).
CREATE TABLE job_dependency (
    parent_version_id BIGINT NOT NULL REFERENCES job_version(id) ON DELETE CASCADE,
    child_version_id  BIGINT NOT NULL REFERENCES job_version(id),
    PRIMARY KEY (parent_version_id, child_version_id)
);

-- ── Contexte global ───────────────────────────────────────────────────────────

CREATE TABLE connection (
    id           BIGSERIAL PRIMARY KEY,
    name         TEXT NOT NULL UNIQUE,
    driver       TEXT NOT NULL CHECK (driver IN ('postgres', 'sqlserver')),
    host         TEXT NOT NULL,
    port         INT NOT NULL,
    database     TEXT NOT NULL,
    username     TEXT NOT NULL,
    secret       BYTEA NOT NULL,        -- chiffré au repos, jamais renvoyé par l'API
    options      JSONB NOT NULL DEFAULT '{}',
    validated_at TIMESTAMPTZ,           -- dernier test de connexion réussi
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE variable (
    name       TEXT PRIMARY KEY,
    value      TEXT,
    secret     BYTEA,                   -- l'un ou l'autre, pas les deux
    is_secret  BOOLEAN NOT NULL DEFAULT false,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CHECK ((is_secret AND secret IS NOT NULL AND value IS NULL)
        OR (NOT is_secret AND value IS NOT NULL AND secret IS NULL))
);

-- ── Planification ─────────────────────────────────────────────────────────────

CREATE TABLE schedule (
    id                     BIGSERIAL PRIMARY KEY,
    job_id                 BIGINT NOT NULL REFERENCES job(id) ON DELETE CASCADE,
    cron                   TEXT NOT NULL,
    timezone               TEXT NOT NULL,   -- IANA, ex. 'Europe/Paris'
    version_pin            BIGINT REFERENCES job_version(id),  -- NULL = dernière publiée
    input_params           JSONB NOT NULL DEFAULT '{}',
    enabled                BOOLEAN NOT NULL DEFAULT true,
    next_run_at            TIMESTAMPTZ,
    overlap_policy         TEXT NOT NULL DEFAULT 'forbid'
                               CHECK (overlap_policy IN ('forbid', 'allow', 'replace')),
    catchup_window_seconds INT NOT NULL DEFAULT 0,  -- 0 = on saute les créneaux manqués
    max_attempts           INT NOT NULL DEFAULT 1   -- retry du run entier (§5.3)
);

CREATE INDEX schedule_due_idx ON schedule (next_run_at) WHERE enabled;

-- ── Exécution ─────────────────────────────────────────────────────────────────

CREATE TABLE run (
    id             BIGSERIAL PRIMARY KEY,
    job_version_id BIGINT NOT NULL REFERENCES job_version(id),
    schedule_id    BIGINT REFERENCES schedule(id) ON DELETE SET NULL,
    parent_run_id  BIGINT REFERENCES run(id),      -- job invoqué comme node
    scheduled_for  TIMESTAMPTZ,                    -- NULL si déclenchement manuel
    trigger        TEXT NOT NULL CHECK (trigger IN ('schedule','manual','retry','parent')),
    attempt        INT NOT NULL DEFAULT 1,
    status         TEXT NOT NULL DEFAULT 'pending',
    input_params   JSONB NOT NULL DEFAULT '{}',
    output_params  JSONB,
    started_at     TIMESTAMPTZ,
    finished_at    TIMESTAMPTZ,
    worker_id      TEXT,
    lease_until    TIMESTAMPTZ,                    -- bail : si dépassé, run interrompu
    triggered_by   TEXT
);

-- LA contrainte d'idempotence. Deux schedulers qui réclament le même créneau :
-- le second se prend une violation d'unicité et l'ignore.
CREATE UNIQUE INDEX run_slot_idx ON run (schedule_id, scheduled_for, attempt)
    WHERE schedule_id IS NOT NULL AND scheduled_for IS NOT NULL;

CREATE INDEX run_claimable_idx ON run (id) WHERE status = 'pending';

-- Trace d'exécution, pas file d'attente (§5.1). Alimente l'UI, le SSE et la télémétrie.
CREATE TABLE node_run (
    id          BIGSERIAL PRIMARY KEY,
    run_id      BIGINT NOT NULL REFERENCES run(id) ON DELETE CASCADE,
    node_id     BIGINT NOT NULL REFERENCES node(id),
    status      TEXT NOT NULL DEFAULT 'pending',
    started_at  TIMESTAMPTZ,
    finished_at TIMESTAMPTZ,
    rows_in     BIGINT NOT NULL DEFAULT 0,
    rows_out    BIGINT NOT NULL DEFAULT 0,
    rows_error  BIGINT NOT NULL DEFAULT 0,
    bytes_peak  BIGINT,
    -- Borne de progression atteinte, pour la reprise incrémentale (§5.3).
    -- NULL si le node n'est pas reprenable.
    checkpoint  JSONB,
    error       TEXT,
    UNIQUE (run_id, node_id)
);

CREATE TABLE node_run_log (
    node_run_id BIGINT NOT NULL REFERENCES node_run(id) ON DELETE CASCADE,
    seq         BIGINT NOT NULL,
    stream      TEXT NOT NULL CHECK (stream IN ('stdout', 'stderr', 'system')),
    at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    line        TEXT NOT NULL,
    PRIMARY KEY (node_run_id, seq)
);

-- ── Build du node Script ──────────────────────────────────────────────────────

CREATE TABLE build (
    id            BIGSERIAL PRIMARY KEY,
    language      TEXT NOT NULL,
    source_hash   TEXT NOT NULL,        -- sha256(source) : la clé du cache
    status        TEXT NOT NULL,
    artifact_path TEXT,
    logs          TEXT,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    finished_at   TIMESTAMPTZ,
    UNIQUE (language, source_hash)
);
```

### Statuts

- `run.status` : `pending` → `running` → `succeeded` | `failed` | `interrupted` | `cancelled`
- `node_run.status` : `pending` → `running` → `succeeded` | `failed` | `skipped`

`interrupted` est distinct de `failed` : le job n'a pas échoué, le processus est mort.
La différence compte pour l'alerting comme pour le retry.

---

## 5. Sémantique d'exécution

### 5.1 Un run, un processus

**Correction d'un choix antérieur.** Une version précédente de ce document faisait des
lignes `node_run` la file d'attente, avec un compteur `pending_parents` et une
réclamation nœud par nœud en `FOR UPDATE SKIP LOCKED`. Ce design était juste pour des
nodes ne communiquant que par effets de bord. Il ne l'est plus : si le node `D`
consomme le `Dataset` produit par `C`, et que ce dataset vit en mémoire, alors `C` et
`D` doivent tourner **dans le même processus**. On ne réclame pas un node, on réclame
un run.

Le modèle retenu :

- le **scheduler** réclame les runs dus (`FOR UPDATE SKIP LOCKED` sur `run`), pose un
  bail et le renouvelle par heartbeat ;
- le **moteur** exécute le graphe **en mémoire dans ce processus** : file de nodes prêts,
  Kahn à l'exécution, nodes indépendants en parallèle via `tokio` + sémaphore ;
- les lignes `node_run` sont écrites au fil de l'eau pour l'UI, le SSE et la télémétrie.
  Elles n'ordonnancent rien.

Ce qu'on perd : un run ne peut pas être réparti sur plusieurs machines. Pour une
plateforme mono-serveur c'est sans conséquence, et les runs restent parallèles entre eux.
Ce qu'on gagne : un moteur beaucoup plus simple, et l'inspection des datasets
intermédiaires devient possible.

### 5.2 Propagation

Un node terminé libère ses successeurs. Pour les arêtes de **données**, la règle est
simple : l'enfant démarre quand tous ses ports d'entrée requis ont reçu leur dataset.

Pour les arêtes de **contrôle**, l'arête est **prise** si sa condition correspond au
statut du parent, **morte** sinon :

| condition | parent `succeeded` | `failed` | `skipped` |
|---|---|---|---|
| `on_success` | prise | morte | morte |
| `on_failure` | morte | prise | morte |
| `always`     | prise | prise | **prise** |

Puis, quand toutes les arêtes entrantes d'un node sont résolues :

- `join_policy = 'all'` → il démarre si aucune arête n'est morte, sinon `skipped` ;
- `join_policy = 'any'` → il démarre si au moins une arête est prise, sinon `skipped`.

Un node `skipped` propage à son tour. `always` est le seul moyen de dire « exécute-toi
même si l'amont a été sauté » — c'est ce qu'on veut pour un nettoyage ou une
notification de fin.

**Statut du run** : `failed` dès qu'un node termine en `failed`, même si une branche
`on_failure` a rattrapé le coup. Un node `skipped` n'est pas un échec. Alternative
écartée : « le run réussit si toutes les feuilles atteignables ont réussi » — plus juste
conceptuellement, imprédictible à lire dans une liste d'historique.

### 5.3 Reprise sur incident, et pourquoi le rejeu intégral ne suffit pas

Un processus qui meurt emporte les lots en cours. Le reaper détecte les baux expirés et
marque le run `interrupted` ; s'il reste des tentatives, il crée un **nouveau run**
`attempt + 1`.

```sql
UPDATE run SET status = 'interrupted', worker_id = NULL
WHERE status = 'running' AND lease_until < now();
```

Sur un job de quelques milliers de lignes, ce nouveau run repart de zéro et c'est très
bien. **Sur 900 millions de lignes, non** : un job de trois heures qui échoue à 95 % ne
peut pas recommencer au début. À cette volumétrie, la reprise doit être **incrémentale**.

**Points de reprise.** Le mécanisme est volontairement minimal, et il est *conditionnel* :

- le node source déclare une **colonne de progression** monotone (clé primaire,
  horodatage de modification) et trie dessus ;
- à chaque tranche commitée, le worker écrit la borne atteinte dans
  `node_run.checkpoint` ;
- au run suivant, la source reprend à `WHERE cle > derniere_borne`.

Trois conditions, et si l'une manque la reprise incrémentale est indisponible — l'API le
dit alors explicitement plutôt que de le laisser deviner :

| Condition | Pourquoi |
|---|---|
| Une colonne de progression monotone | sans ordre stable, « reprendre après » n'a pas de sens |
| Mode d'Output `Upsert` | une tranche peut avoir été commitée deux fois |
| Aucun node bloquant dans la chaîne | un bloquant n'a pas de notion de progression partielle |

> **La garantie est *au moins une fois*, pas *exactement une fois*.**
> Un run rejoué, en entier ou depuis un point de reprise, refait des écritures. C'est aux
> jobs d'être rejouables, et c'est précisément le rôle du mode d'un node SQL Output :
>
> | Mode | Rejouable | Reprise incrémentale |
> |---|---|---|
> | `Upsert` | oui — par la clé de conflit | **oui** |
> | `Truncate` puis insertion | oui — l'état final ne dépend pas du nombre d'exécutions | non — la table repart vide |
> | `Insert` | non — échoue sur doublon, ce qui est le bon signal | non |
>
> Choisir le mode d'un Output, c'est répondre à « ce job est-il rejouable, et
> reprenable ». À afficher comme tel dans l'éditeur, pas comme une option technique
> parmi d'autres.

### 5.4 Transactions : l'atomicité contre la reprise

Une transaction donne l'**atomicité**, pas l'idempotence — l'idempotence vient de la clé.
Et une transaction ne peut pas couvrir tout un job dès qu'il touche deux bases : il n'y a
pas de transaction distribuée ici, et il n'en est pas question. **Portée maximale : un
node SQL Output.** Un job avec deux Outputs a deux transactions indépendantes ; si la
seconde échoue, la première reste commitée. Contre-intuitif, donc à afficher dans l'UI.

Mais à 900 millions de lignes, même une transaction par node ne tient pas : le WAL
PostgreSQL ou le journal SQL Server grossit jusqu'à saturer le disque, et rien n'est
visible avant la fin. D'où un arbitrage explicite par node (**D18**) :

| `commit_mode` | Atomicité | WAL / journal | Progression | Reprise |
|---|---|---|---|---|
| `single` | totale — tout ou rien | croît avec le volume | invisible | non |
| `chunked` (défaut, 50 000 lignes) | par tranche | borné | visible | **oui** |

`chunked` est le défaut parce qu'à cette échelle c'est le seul mode praticable, mais le
choix doit être **affiché comme une perte d'atomicité assumée** : un run interrompu en
`chunked` laisse une table partiellement chargée. C'est acceptable en `Upsert` (l'état
converge au run suivant), et c'est précisément pour ça que les deux réglages sont liés.

Le mode `Truncate` a un problème propre : vider une table de 900 millions de lignes puis
la recharger pendant trois heures la laisse **vide ou incomplète** pendant toute la
durée du job, alors que des applications la lisent. La réponse est une **table
intermédiaire suivie d'un échange** (charger à côté, puis renommer) — détaillé dans
[nodes.md](nodes.md) §3.

---

## 6. Le catalogue de nodes

Un `kind` = une variante d'enum Rust + un JSON Schema dérivé par
[`schemars`](https://docs.rs/schemars) + une implémentation du trait `Node`.

```rust
pub trait Node {
    /// Ports d'entrée et de sortie, éventuellement dépendants de la config
    /// (un Map à trois sorties déclare trois ports).
    fn ports(&self) -> (Vec<PortSpec>, Vec<PortSpec>);

    /// Propagation de schéma à la conception, sans exécuter quoi que ce soit.
    fn output_schemas(&self, inputs: &[Schema]) -> Result<Vec<Schema>, SchemaError>;

    /// Ce node a-t-il besoin du dataset entier ? (métadonnée, cf. §2)
    fn is_blocking(&self) -> bool;

    async fn execute(&self, ctx: &Ctx, inputs: Vec<Dataset>) -> Result<Vec<Dataset>>;
}
```

Le front **ne connaît aucun type de node en dur** :

```
GET /api/node-kinds
→ [ { "kind": "sql_input", "family": "sql", "label": "SQL Input",
      "icon": "database", "inputs": [], "outputs": [{"key":"out","kind":"data"}],
      "config_schema": { ...JSON Schema... } }, ... ]
```

Ce détour n'est pas de la sur-ingénierie, c'est la contrepartie assumée d'un front
démarré avant que les nodes existent (roadmap, étapes 10–15). Les nodes SQL arrivent
**après** l'éditeur : leur intégration doit coûter **zéro ligne de React**. C'est un
critère d'acceptation, pas un espoir.

La spécification node par node est dans [nodes.md](nodes.md).

---

## 7. Validation

À la publication d'un draft, dans l'ordre :

1. Les `key` de nodes sont uniques et non vides.
2. Chaque `config` valide contre le JSON Schema de son `kind`.
3. Toute arête référence des nodes et des **ports** existants, de genres compatibles.
4. Exactement un node `Start` et un node `End`.
5. Tout port d'entrée `required` est branché ; un port d'entrée `data` a au plus une
   arête entrante.
6. **Aucun cycle** — tri topologique de Kahn ; s'il reste des nodes, on remonte à
   l'utilisateur **les nodes du cycle**, pas un message générique.
7. **Propagation de schéma** de `Start` vers `End` : chaque `output_schemas` réussit,
   et chaque arête de données relie des schémas compatibles.
8. Pour chaque node `Job` : la version embarquée existe, sa `signature` correspond au
   branchement, et l'ajout ne crée pas de cycle **inter-jobs** (via `job_dependency`,
   avec une profondeur maximale).
9. Nodes inatteignables depuis `Start` : avertissement, pas erreur.

L'API expose ça sans publier, pour l'éditeur :

```
POST /api/jobs/{id}/draft/validate
→ { "port_schemas": { "map1.out": {...} }, "errors": [...], "warnings": [...] }
```

La détection de cycle vit **deux fois** : en Rust à la publication (autorité), et en
TypeScript dans l'éditeur (retour immédiat, empêche de tirer une arête invalide).
Duplication assumée : l'ergonomie exige le feedback local, la fiabilité exige le serveur.

Publier un job dont d'autres jobs dépendent ne les modifie pas — ils épinglent leur
version (**D11**). La liste des dépendants est affichée à la publication, avec une
action explicite de mise à jour.

---

## 8. Décisions

| # | Question | Position | À revoir à |
|---|---|---|---|
| **D1** | Un planning pointe sur une version figée ou sur la dernière ? | `version_pin = NULL` (dernière **publiée**) par défaut. La version résolue est écrite dans `run.job_version_id`, donc l'historique reste exact. Épinglage explicite possible pour les jobs sensibles. | Étape 10 |
| **D2** | Rattrapage après une panne de 6 h ? | `catchup_window_seconds = 0` : on saute et on journalise un créneau manqué, qui déclenche une alerte. Fenêtre par planning là où le rattrapage a du sens. Rejouer 6 h d'un coup est presque toujours une erreur. | Étape 9 |
| **D3** | Changement d'heure : un job à 02h30 ? | Heure inexistante → exécution **sautée** (journalisée). Heure dupliquée → **une seule** exécution, sur la première occurrence. Stockage en `TIMESTAMPTZ`, calcul dans le fuseau du planning. | Étape 2 |
| **D4** | ~~Passage de données entre nodes ?~~ | **Annulée.** La position antérieure (« v1 = flux de contrôle seul ») est invalidée par [nodes.md](nodes.md) : le flux de données est le cœur du produit. Voir §2 et §3. | — |
| **D5** | Rétention des logs ? | 90 jours pour `node_run_log`, 1 an pour `run`/`node_run` sans les lignes. Purge planifiée. Chiffres à confirmer sur le volume réel. | Étape 23 |
| **D6** | Qui a le droit de déployer ? | v1 : rôles `viewer` / `operator`, plus un journal d'audit sur toute mutation (qui, quoi, quand, avant/après). Le par-job attendra un besoin réel. | Étape 10 |
| **D7** | Dépendances Go : accès réseau du serveur de build ? | À trancher **avec la DSI**, c'est réseau autant que technique. Défaut prudent : `GOPROXY` interne ; à défaut, build hors ligne et vendoring. | Étape 13 |
| **D8** | Graphe en Rust : à la main ou `petgraph` ? | À la main à l'étape 3 (arène `Vec<Node>` + `HashMap<NodeId, Vec<NodeId>>`). L'objectif est de comprendre pourquoi `Rc<RefCell<Node>>` est un piège. `petgraph` si les algorithmes se multiplient. | Étape 3 |
| **D9** | Langage d'expression du node `Map` ? | **Remplacée par D17.** La position antérieure (Rhai, évalué **par ligne**) était mauvaise à cette volumétrie : 315 milliards d'appels d'interpréteur. Les expressions DataFusion (`Expr`) sont **vectorisées sur les colonnes Arrow**, et le langage exposé à l'utilisateur peut être du SQL — que les gens connaissent déjà, contrairement à Rhai. | — |
| **D10** | Datasets matérialisés ou en flux ? | **Inversée.** La position antérieure (« matérialisés en v1, le flux plus tard ») est intenable à 900 M × 350 : elle plafonne à ≈ 1,4 M de lignes (§2.1). **Le flux de lots Arrow est le mode v1**, la mémoire de pointe est bornée par `taille_de_lot × profondeur_du_pipeline`. Aucun node bloquant en v1 hors `Script`. | Étape 4 |
| **D11** | Quelle version quand un job embarque un job ? | **Figée à l'insertion**, avec une action « mettre à jour » explicite. Sinon republier B modifie A en silence. La signature de B est vérifiée à la publication de A. | Étape 20 |
| **D12** | SQL Server en Rust ? | **Révisée le 2026-08-03 : `arrow-odbc`, pas `tiberius`.** `sqlx` a retiré MSSQL en 0.7 et ne couvre plus que PostgreSQL, MySQL et SQLite ; il reste pour PostgreSQL, avec ses macros vérifiées à la compilation. Pour SQL Server, [`tiberius`](https://docs.rs/tiberius) est **sans release depuis juillet 2024** — et surtout [`arrow-odbc`](https://docs.rs/arrow-odbc) est meilleur techniquement : il lit et écrit **directement des `RecordBatch`** (pas de couche ligne → colonne à écrire), apporte le chargement en masse, et ouvre toutes les sources ODBC pour un coût nul. Prix : le driver ODBC Microsoft devient une dépendance système. Détail et chiffres dans [dependances.md](dependances.md) §3.2. | Étape 17 |
| **D13** | Déclenchement par webhook ? | Reporté (**H1**). Ce n'est pas un node mais un **déclencheur**, et ça change le modèle de planification : un job ne serait plus seulement cron. À traiter comme une famille de déclencheurs (cron, webhook, fichier déposé) une fois le cron solide. | Après v0.5 |
| **D14** | Erreur sur une ligne : on meurt ou on continue ? | Trois politiques par node — `die` (défaut), `ignore`, `reject` vers un port d'erreur (§3). `reject` est ce qui rend le produit utilisable en production. | Étape 5 |
| **D15** | D'où viennent les fichiers ? | Chemins locaux et partages UNC en v1, avec une **liste blanche de racines autorisées** en configuration — un node ne doit pas pouvoir lire `C:\Windows`. Le compte de service porte les droits. SFTP hors v1. | Étape 18 |
| **D16** | Le mode `script` a-t-il son propre moteur ? | **Non.** Graphe synthétisé `Start → Command`\|`Script → End` (§1.1). Un chemin d'exécution parallèle coûterait deux schedulers, deux modèles de retry et deux pipelines de logs, pour un gain nul. Le mode `script` est une **vue d'édition**, pas un runtime. | Étape 12 |
| **D17** | Arrow seul, ou DataFusion ? | **Renversée : DataFusion dès le départ.** La position antérieure (« opérateurs écrits à la main ») s'appuyait en partie sur un argument d'apprentissage écarté depuis — réécrire des tris et jointures hors-mémoire n'est pas un exercice, c'est un projet de plusieurs années fait moins bien. DataFusion est de fait le standard pour un moteur d'exécution Arrow en Rust. **Mais il assure le plan de données, pas le plan de contrôle** (§2.2) : le graphe compile en *plusieurs* plans, découpés aux frontières d'effet de bord, et ntz garde l'orchestration, le retry et les points de reprise. | Étape 4 |
| **D19** | UI en Rust/WASM, ou React ? | **React conservé, types générés depuis Rust.** L'option Rust a été instruite : côté DOM il n'existe aucune bibliothèque exploitable (`flow-rs-leptos` est en 0.1.0-beta, dormante depuis 11 mois) ; `egui` + `egui-snarl` est réellement viable pour le canvas, mais le canvas n'est que ~30 % de l'interface, et egui est le plus faible exactement là où on livre d'abord — l'éditeur de code du mode `script` (v0.5), où CodeMirror n'a pas d'équivalent. La redondance de types, qui était le vrai argument contre React, est traitée par la génération (§9.1). | Étape 10 |
| **D18** | Une transaction unique, ou des commits par tranches ? | **`chunked` par défaut, 50 000 lignes.** Une transaction unique sur 900 M d'insertions sature le WAL et ne montre aucune progression. Le prix est réel — un run interrompu laisse une table partiellement chargée — donc le réglage est **affiché comme une perte d'atomicité assumée**, et lié au mode `Upsert` qui le rend convergent (§5.4). `single` reste disponible pour les petits volumes. | Étape 17 |

---

## 9. Déploiement

Un seul exécutable Windows. Le front React est compilé par Vite en fichiers statiques,
embarqué dans le binaire (`rust-embed`) et servi par axum. Pas de serveur web séparé,
pas de Node.js en production, pas de fichiers à synchroniser.

```
ntz.exe ──┬── service Windows (windows-service)
          ├── scheduler : réclame les runs dus, pose les baux
          ├── moteur    : exécute un graphe par run, en mémoire
          ├── reaper    : marque interrompus les runs à bail expiré
          ├── axum      : API JSON + SSE + assets React embarqués
          └── /metrics  : Prometheus → Grafana
                              │
                        PostgreSQL (métadonnées)
                              │
                   bases cibles : PostgreSQL, SQL Server
```

Scheduler, moteur et API dans un même processus en v1 : suffisant, et le découpage
reste possible sans changement de schéma, puisque la coordination passe par la base.

### 9.1 Types générés depuis Rust (D19)

Rust est la source unique des formes de données ; le front ne redéclare rien.

**Mécanisme retenu : l'OpenAPI, déjà prévu à l'étape 10.** `utoipa` produit le schéma
depuis les handlers axum, et le front en génère ses types — plus, si on le souhaite, son
client `fetch` et ses hooks TanStack Query. L'intérêt de passer par là plutôt que par
[`ts-rs`](https://docs.rs/ts-rs) ou [`specta`](https://docs.rs/specta) : une seule
annotation à maintenir au lieu de deux, et la documentation d'API en prime. `ts-rs`
reste le repli si l'annotation `utoipa` devient pénible.

**La génération doit être vérifiée en CI, pas lancée à la main.** Un fichier généré qui
dérive est pire que pas de génération, parce que tout le monde lui fait confiance. Donc :
générer dans `web/src/types/generated/`, committer le résultat, et **faire échouer la CI
si régénérer produit un diff**.

> **Le piège : ne pas typer statiquement les configs de nodes.** Il est tentant de générer
> aussi les types de `NodeConfig`. Ce serait détruire la propriété obtenue en §6 — le
> front ne connaît aucun type de node en dur, il lit `GET /api/node-kinds` et rend les
> formulaires depuis le JSON Schema. Ajouter le node SQL après l'éditeur doit coûter zéro
> ligne de React ; le typer statiquement rétablit le couplage.
>
> Règle : **types générés pour l'enveloppe d'API et les entités** (job, run, node_run,
> schedule, connection, réponse de validation, erreurs) ; **payload de config en
> `unknown`, piloté par le schéma**. Exception assumée pour les rares configs que le front
> manipule spécifiquement — `Map` et `Script`, qui ont leur propre éditeur.

**Ce que les types ne résolvent pas.** Ils garantissent les *formes*, pas les *règles*.
La détection de cycles, la compatibilité de schémas de ports et la normalisation de noms
de l'auto-mapping sont du comportement, et restent dupliquées si on les réécrit en
TypeScript.

Réponse v1, la moins chère : **aucune règle réimplémentée côté client**, hors une
détection de cycle locale minimale pour refuser une arête à la souris. Tout le reste passe
par `POST /api/jobs/{id}/draft/validate` (étape 10) appelé en debounce — l'autorité reste
le serveur, et c'est déjà ce que §7 prévoit. Compiler le cœur de validation en WASM
(`arrow-schema` suffit côté navigateur, pas `arrow` entier) reste possible plus tard sans
remettre en cause ce choix : les deux se composent.

### 9.2 Ordre de construction

Le binaire unique impose un enchaînement que ni `cargo build` ni `vite build` ne
connaissent seuls :

```
cargo  → OpenAPI  →  génération TS  →  vite build  →  rust-embed  →  ntz.exe
```

Un `build.rs` qui orchestre tout ça devient vite fragile. Préférer un
[`cargo xtask`](https://github.com/matklad/cargo-xtask) ou un `justfile` avec des étapes
explicites et réexécutables — d'autant que la production ne doit contenir **aucun
Node.js** : le front est buildé à la construction, jamais sur le serveur.

**Secrets.** Les `connection.secret` et `variable.secret` sont chiffrés au repos. La
clé maîtresse vient de DPAPI (liée au compte de service) ou d'un fichier à ACL
restreinte. L'API ne renvoie **jamais** un secret déchiffré, et le masquage s'applique
aux logs **et au flux SSE**. Un DSN complet avec mot de passe posé en dur sur un node
est refusé à la publication : un node référence une `connection`, il ne la contient pas.
