# ntz — Roadmap d'apprentissage

Triple objectif : livrer une plateforme de jobs planifiés, apprendre Rust en l'écrivant,
et apprendre React en construisant l'éditeur de nodes.

Les étapes sont ordonnées par **progression d'apprentissage**, pas par importance
fonctionnelle. Chaque étape est autonome et donne quelque chose qui tourne.

Runtime cible v1 : **Go**. Plateforme v1 : **Windows Server**. Métadonnées :
**PostgreSQL**. Bases cibles des jobs : **PostgreSQL et SQL Server**.

- Modèle de données et sémantique d'exécution : [architecture.md](architecture.md)
- Spécification node par node : [nodes.md](nodes.md)
- Découpage en crates et traits : [traits.md](traits.md)
- Dépendances, maintenance, coûts : [dependances.md](dependances.md)
- Transition Angular → React : [react-depuis-angular.md](react-depuis-angular.md)

---

## Ce qu'on construit

Un job est un **graphe de nodes entre lesquels circule de la donnée** : un ETL visuel,
pas un cron amélioré. Deux modes d'édition, **un seul moteur** (architecture.md §1.1) :

| Mode | Édition | Graphe |
|---|---|---|
| `script` | un éditeur de code, ou une commande | `Start → Command`\|`Script → End`, synthétisé |
| `graph` | le canvas de nodes | dessiné |

**La contrainte qui dimensionne tout : jusqu'à 900 millions de lignes de 350 champs**, soit
≈ 315 milliards de cellules et de l'ordre de 6 To. Ce n'est pas une volumétrie faible, et
elle n'est pas un détail d'optimisation à traiter à la fin — elle décide de la
représentation des données (Arrow colonnaire en flux, étape 4), du mode d'écriture en base
(chargement en masse obligatoire, étape 17), du modèle de reprise (points de reprise,
étape 9) et de l'ergonomie du `Map` (auto-mapping, étape 19). Le détail du calcul est en
[architecture.md §2.1](architecture.md).

```
Socle Rust      0 ─ 1 ─ 2 ─ 3
Données                       4 ─ 5
Fiabilité                           6 ─ 7 ─ 8 ─ 9
API & front                                     10 ─ 12 ─ 13 ══ v0.5 ══ 14 ─ 15
React (parallèle)      ╰────────── 11 ──────────╯
Nodes                                                                         16 ─ 17 ─ 18 ─ 19
Plateforme                                                                                    20 ─ 21 ─ 22 ─ 23
```

**Le jalon v0.5 (après l'étape 13) est le vrai objectif de court terme.** Le mode
`script` remplace Task Scheduler sans avoir besoin d'un seul node SQL, ni du canvas, ni
du `Map`. À partir de là, on migre les exe historiques un par un et on gagne déjà ce que
Task Scheduler ne donne pas : historique, logs conservés, alerte sur run manqué,
télémétrie. Le mode `graph` se construit ensuite, sur une plateforme déjà en production.

**L'étape 11 (fondations React) ne dépend d'aucun backend** — c'est de l'apprentissage
pur, à démarrer dès l'étape 3, en parallèle, les soirs où le borrow checker a gagné.

> **Ordre alternatif, si l'urgence métier prime.** Les étapes 4, 5 et 7 ne servent que
> le mode `graph`. Les sauter (`0-3, 6, 8, 9, 10, 11, 12, 13`) avance le jalon v0.5 de
> plusieurs semaines. Le coût : on perd l'enchaînement d'apprentissage graphe → dataflow
> → DAG parallèle, et l'étape 7 devient beaucoup plus dure à attaquer seule.
> Je recommande l'ordre nominal, mais c'est un arbitrage, pas une évidence.

---

# Socle Rust

## Étape 0 — Fondations Rust

Ne pas sauter. Les étapes suivantes supposent ces bases acquises.

- [ ] The Rust Book, chapitres 1–10 (ownership, borrowing, structs, enums, traits, génériques)
- [ ] Rustlings jusqu'à la section `traits`
- [ ] Chapitre 9 en particulier : `Result`, `Option`, l'opérateur `?`

**Critère d'acceptation :** savoir expliquer pourquoi `let s2 = s1;` invalide `s1` pour
une `String` mais pas pour un `i32`.

---

## Étape 1 — Un CLI qui lance un process

Pas de base, pas de web, pas d'async. Du Rust synchrone.

- [ ] Workspace Cargo, premiers crates `ntz-cli` et `ntz-domain`
- [ ] `ntz run <commande>` exécute et affiche stdout/stderr
- [ ] Propager le code de sortie du process enfant
- [ ] `anyhow` dans le binaire, `thiserror` dans les libs, aucun `unwrap()`
- [ ] `Cargo.lock` committé, et **`cargo-deny` en CI dès maintenant** — mis en place à
      l'étape 1 il ne coûte rien, à l'étape 20 il révèle un problème qu'on ne peut plus
      défaire (dependances.md §9)

**Concepts :** `std::process::Command`, `Result`, `?`, `clap` (derive), `anyhow`

> **Ne pas créer les dix crates de [traits.md](traits.md) tout de suite** — ce serait dix
> `Cargo.toml` vides à maintenir. Les extraire quand un besoin réel apparaît. En revanche,
> `ntz-domain` dès maintenant, parce que la règle qui compte (« il ne dépend de rien ») ne
> s'installe jamais rétroactivement.

**Critère d'acceptation :** un exécutable qui échoue fait échouer `ntz run` avec le même code.

---

## Étape 2 — Configuration et expressions cron

- [ ] `jobs.toml` désérialisé en structs Rust
- [ ] `ntz next <job>` affiche les 5 prochaines exécutions
- [ ] Fuseau explicite par job (`Europe/Paris`)
- [ ] Implémenter **D3** : heure inexistante → sautée ; heure dupliquée → une seule fois

**Concepts :** `serde` + derive, `toml`, `chrono`, `chrono-tz`, crate `cron` ou `croner`
**Critère d'acceptation :** un test unitaire aux deux dates de bascule horaire, avec les
dates en dur — jamais `now()` dans un test.

---

## Étape 3 — Le graphe en mémoire

Le job devient un DAG. Toujours **synchrone** : on isole la difficulté « graphe » de la
difficulté « async ». C'est ici qu'on comprend vraiment l'ownership.

- [ ] `jobs.toml` décrit des nodes et des arêtes
- [ ] Graphe en **arène** : `Vec<Node>` + `HashMap<NodeId, Vec<NodeId>>`, `NodeId` en newtype
- [ ] Tri topologique de Kahn, détection de cycle qui **nomme les nodes fautifs**
- [ ] `ntz plan <job>` affiche l'ordre d'exécution et les paliers de parallélisme

**Concepts :** `HashMap`/`HashSet`, newtypes, pattern arène, `impl Display`, `thiserror`

> **Pourquoi une arène et pas `Rc<RefCell<Node>>`.** Le réflexe venu des langages à GC
> est de faire pointer les nodes les uns sur les autres. En Rust ça compile, puis ça
> devient un enfer : emprunts imbriqués, panics `already borrowed` à l'exécution, cycles
> de références qui fuient. Indexer une `Vec` par un `NodeId` supprime le problème.
> Cette étape sert précisément à ancrer ce réflexe. Voir **D8**.

**Critère d'acceptation :** un graphe cyclique est rejeté avec la liste des nodes du
cycle. Un losange (A→B, A→C, B→D, C→D) donne un ordre valide, `D` toujours en dernier.

---

# Le modèle de données

## Étape 4 — Arrow : schémas, lots, flux

La brique dont `Map`, la validation de branchement et l'autocomplétion dépendent toutes.
Toujours synchrone, et sans graphe.

C'est ici que la volumétrie décide de tout. Une représentation orientée ligne
(`Row(Vec<Value>)`) plafonne à **≈ 1,4 million de lignes** à 350 colonnes sur 16 Go de
RAM — le calcul est en architecture.md §2.1. On part donc en **colonnes, par lots, en
flux** : `arrow-rs`, `RecordBatch`, mémoire de pointe indépendante du volume total.

- [ ] Schémas Arrow : `Field`, `Schema`, `DataType`, métadonnées
- [ ] **`Decimal128` pour tout montant**, jamais `Float64`
- [ ] Construire des `RecordBatch` avec les *builders*, lire les colonnes en typé
- [ ] Conversions (`arrow::compute::cast`) : élargissement autorisé, rétrécissement refusé
- [ ] Compatibilité de schémas, avec un message qui nomme la colonne fautive
- [ ] Lire/écrire un CSV **en flux de lots**, options françaises comprises (`;`, cp1252,
      virgule décimale, `%d/%m/%Y`, BOM absorbé)
- [ ] Premier contact DataFusion : `SessionContext`, l'API `DataFrame`, exécuter une
      requête sur un CSV et consommer le `SendableRecordBatchStream`
- [ ] Plafonner le `MemoryPool` et observer un déversement disque se produire
- [ ] Mesurer : débit en lignes/s et mémoire de pointe sur un fichier large

**Concepts :** `arrow-rs`, tampons et bitmaps de validité, zéro-copie, `Stream` et
`Iterator`, `encoding_rs`, `SessionContext`, `MemoryPool`

**À lire, pas seulement à taper :**

- le [guide d'architecture DataFusion](https://datafusion.apache.org/library-user-guide/index.html) —
  la distinction `LogicalPlan` / `ExecutionPlan` conditionne l'étape 5
- le trait [`ExecutionPlan`](https://docs.rs/datafusion/latest/datafusion/physical_plan/trait.ExecutionPlan.html)
  et l'implémentation de `FilterExec` : c'est court, et ça montre comment un opérateur
  en flux est réellement écrit
- la [doc du format Arrow](https://arrow.apache.org/docs/format/Columnar.html) sur la
  disposition colonnaire et les bitmaps de validité — vingt minutes qui rendent tout le
  reste évident

> La donnée d'un groupe de concessions est en grande partie financière. `0.1 + 0.2 != 0.3`
> en binaire, et une erreur d'arrondi sur une marge n'est pas un bug qu'on découvre vite.

**Critère d'acceptation :** lire un CSV de 350 colonnes et 5 millions de lignes, le
retyper, le réécrire — avec une **mémoire de pointe qui ne dépend pas du nombre de
lignes**. C'est le critère qui compte : si la mémoire croît avec le fichier, le flux n'est
pas réellement en flux, et tout ce qui suit s'effondrera à l'échelle réelle.

---

## Étape 5 — Compiler le graphe en plans DataFusion

Le graphe ne devient pas *un* plan, mais **plusieurs tronçons découpés aux frontières
d'effet de bord** (architecture.md §2.2). C'est l'étape qui matérialise la frontière
plan de données / plan de contrôle, et c'est le cœur conceptuel du projet.

- [ ] Les crates `ntz-domain` et `ntz-ports`, avec la règle de dépendance appliquée
      (traits.md §1) : ni `datafusion`, ni `sqlx`, ni `arrow-odbc` dedans
- [ ] Trait `NodeKind` et l'enum `Execution` : `Relational(op)` ou `SideEffect` — c'est
      cette méthode, et non un `execute()` générique, qui matérialise la frontière
      (traits.md §3.2)
- [ ] `Predicate` = **fragment SQL validé**, pas un AST — l'engine le parse avec
      l'analyseur de DataFusion (traits.md §3.2)
- [ ] `TableProvider` pour les sources (`File Input` d'abord), avec **descente de
      prédicats** : le filtre doit remonter dans la source, pas être appliqué après
- [ ] `Filter` et `Map` palier 1 traduits en `Expr` DataFusion, **pas** en code par ligne
- [ ] Nodes `Start`, `End`, `Log`, `Command`
- [ ] **Le découpeur** : parcourir le graphe, isoler les tronçons purs, les compiler,
      et laisser l'orchestrateur enchaîner ce qui les sépare
- [ ] **Propagation de schéma** de `Start` à `End` avant toute exécution — c'est ntz qui
      la fait, pour pouvoir la refuser à la conception
- [ ] Arêtes **données** et **contrôle** (architecture.md §3), politiques d'erreur
      `die` / `ignore` / `reject` + port `error` (**D14**)

**Concepts :** objets-traits, `Arc`, `TableProvider`, `ExecutionPlan`, `Expr`,
`SendableRecordBatchStream`, contre-pression

**À lire :** l'exemple [`custom_datasource`](https://github.com/apache/datafusion/blob/main/datafusion-examples/examples/custom_datasource.rs)
et [`advanced_parquet_index`](https://github.com/apache/datafusion/tree/main/datafusion-examples)
du dépôt DataFusion. Le premier est presque exactement ce que doit être un node source.

**Critère d'acceptation triple.** Fonctionnel : `CSV → Filter → Map → CSV` tourne, et une
incompatibilité de schéma est **refusée avant exécution** avec le nom du port et des
colonnes fautives. **Échelle :** le même job sur 50 M de lignes consomme la même mémoire
que sur 50 000. **Découpage :** insérer un node à effet de bord au milieu de la chaîne
produit deux plans et non un, vérifiable en inspectant les plans générés — c'est ce qui
prouve que la frontière est réelle et pas seulement écrite dans un document.

---

# Async et fiabilité

## Étape 6 — La boucle de planification

Le saut vers l'async. L'exécution du graphe reste séquentielle : on ne change qu'une
chose à la fois.

- [ ] Passage à `tokio`, `#[tokio::main]`
- [ ] Boucle de tick qui déclenche les jobs dus
- [ ] Plusieurs jobs en parallèle entre eux
- [ ] Arrêt propre sur Ctrl-C : on n'abandonne pas un run en cours

**Concepts :** `async`/`await`, `tokio::spawn`, `tokio::time::interval`,
`tokio::select!`, `Arc<Mutex<_>>`, `CancellationToken`

**Critère d'acceptation :** deux jobs planifiés à la même minute démarrent réellement en
parallèle. Un Ctrl-C attend la fin du run en cours avant de rendre la main.

---

## Étape 7 — Exécution parallèle du DAG

Kahn, mais à l'exécution.

- [ ] File des nodes prêts ; un node terminé libère ses successeurs
- [ ] Fan-out, fan-in, `join_policy` `all` / `any`
- [ ] Conditions d'arête `on_success` / `on_failure` / `always`
- [ ] Propagation de `skipped` selon la table de architecture.md §5.2
- [ ] Limite de parallélisme global (sémaphore)

**Concepts :** `tokio::sync::mpsc`, `JoinSet`, `FuturesUnordered`, `Semaphore`

**Critère d'acceptation :** sur un losange dont B échoue — `D` en `join_policy = all` est
`skipped`, un node branché sur B en `on_failure` s'exécute, un node branché en `always`
sur D s'exécute malgré le skip. Trois tests, un par ligne du tableau.

---

## Étape 8 — Logs, timeout, recouvrement

- [ ] `tokio::process`, stdout/stderr en pipes
- [ ] Lecture ligne par ligne au fil de l'eau, pas à la fin du process
- [ ] Timeout par node
- [ ] Recouvrement par planning : `forbid` (défaut) / `allow` / `replace`

**Concepts :** `tokio::io::AsyncBufReadExt`, `tokio::sync::mpsc`, `tokio::time::timeout`

**Critère d'acceptation :** un script qui écrit une ligne par seconde pendant 10 s
affiche ses lignes en temps réel, pas d'un bloc à la fin.

> À ce stade le timeout ne tue que le process direct, pas ses enfants. Étape 22.

---

## Étape 9 — Persistance

- [ ] PostgreSQL en Docker, `sqlx` avec migrations
- [ ] Le schéma de architecture.md §4, `job_draft` inclus
- [ ] `UNIQUE (schedule_id, scheduled_for, attempt)` — l'idempotence des créneaux
- [ ] **Le `run` est l'unité de réclamation** (`FOR UPDATE SKIP LOCKED` sur `run`), pas
      le node : les datasets vivent en mémoire, donc un run tient dans un processus
      (architecture.md §5.1)
- [ ] `node_run` écrit au fil de l'eau pour l'UI et la télémétrie — il n'ordonnance rien
- [ ] Bail + heartbeat + reaper qui marque `interrupted` les runs à bail expiré
- [ ] Retry du run, `attempt + 1` (architecture.md §5.3)
- [ ] **Points de reprise** : `node_run.checkpoint`, borne de progression écrite à chaque
      tranche commitée. Un job de trois heures qui échoue à 95 % ne peut pas recommencer
      au début — c'est la volumétrie qui l'impose, pas le confort
- [ ] L'API dit **explicitement** qu'un job n'est pas reprenable, et pourquoi (les trois
      conditions de architecture.md §5.3)
- [ ] Créneaux manqués après redémarrage, politique **D2**

**Concepts :** `sqlx` (macros vérifiées à la compilation), transactions, isolation, pool

**Critère d'acceptation :** tuer le service à 60 % d'un job de 10 millions de lignes en
`Upsert` avec `progress_column`, puis le relancer — il reprend à la dernière borne
commitée, et l'état final est identique à celui d'un run non interrompu. Sur un job sans
`progress_column`, il repart de zéro **et l'interface l'avait annoncé avant**.

---

# API et front

## Étape 10 — API HTTP

Volontairement minimale, mais avec le **contrat de nodes figé dès maintenant** : c'est ce
qui protège le front des dix étapes suivantes.

- [ ] `axum` : CRUD jobs, draft, publication, plannings
- [ ] `GET /api/node-kinds` — catalogue + JSON Schema générés par `schemars`
- [ ] `POST /api/jobs/{id}/draft/validate` — schémas de ports, erreurs, avertissements,
      **sans publier**
- [ ] Historique des runs, détail avec l'état de chaque node, logs
- [ ] Déclenchement manuel
- [ ] Authentification, rôles `viewer` / `operator` (**D6**), journal d'audit
- [ ] Erreurs en JSON structuré (`{ code, message, details }`)
- [ ] OpenAPI (`utoipa`) : le front **génère** ses types, il ne les recopie pas (**D19**)
- [ ] La CI **échoue si régénérer les types produit un diff** — un fichier généré qui
      dérive est pire que pas de génération, parce qu'on lui fait confiance
- [ ] Ne **pas** typer statiquement les configs de nodes : elles restent pilotées par le
      JSON Schema, sinon le découplage de l'étape 17 est perdu (architecture.md §9.1)

**Concepts :** `axum`, extracteurs, `State` via `Arc`, `IntoResponse`, middleware `tower`,
`schemars`

**Critère d'acceptation :** tout ce que fera l'UI passe par l'API, sans raccourci. Et
`GET /api/node-kinds` décrit les nodes assez complètement pour qu'un client génère les
formulaires de configuration sans rien savoir de ntz.

---

## Étape 11 — React : fondations depuis Angular

Aucune dépendance backend. À démarrer dès l'étape 3, en parallèle.

Le tooling et TypeScript sont déjà acquis. Le travail réel est de **désapprendre** : pas
de DI, pas de RxJS, pas de change detection, pas de two-way binding. Détail dans
[react-depuis-angular.md](react-depuis-angular.md).

- [ ] Vite + React + TypeScript, ESLint, Prettier
- [ ] JSX : `*ngIf` → `&&`, `*ngFor` → `.map()` + `key`, `ng-content` → `children`
- [ ] `useState` et **l'immutabilité obligatoire** : muter un état ne redessine rien
- [ ] Composants contrôlés : `[(ngModel)]` n'existe pas
- [ ] `useEffect` — et surtout **quand ne pas l'utiliser**
- [ ] Remonter l'état, `useContext` pour le vraiment transverse
- [ ] `useMemo` / `useCallback` / `memo` : le modèle de re-rendu
- [ ] TanStack Query pour l'état serveur
- [ ] Vitest + React Testing Library

**Critère d'acceptation double.** Un CRUD branché sur l'API de l'étape 10 **sans un seul
`useEffect` pour aller chercher des données** — si `useEffect` sert à faire un fetch,
TanStack Query n'a pas été comprise. **Et : aucun type de l'API écrit à la main dans
`web/`.** Renommer un champ dans une struct Rust doit casser la compilation du front, pas
se découvrir à l'exécution.

---

## Étape 12 — Front mode `script`

Pas de canvas. Le chemin le plus court vers quelque chose d'utilisable en production.

- [ ] Liste des jobs, création d'un job `script`, édition d'un planning
- [ ] Éditeur de code (CodeMirror 6) ou champ commande + arguments
- [ ] Historique des runs, consultation des logs, déclenchement manuel
- [ ] Le mode `script` publie le graphe synthétisé (**D16**) — l'utilisateur ne le voit pas
- [ ] Front buildé et servi par axum (`rust-embed`)

**Critère d'acceptation :** planifier un exe historique via le node `Command`, le voir
tourner, consulter ses logs trois jours plus tard. Aucun terminal, aucun Task Scheduler.

---

## Étape 13 — Pipeline de build Go

Ce qui transforme « planifier un exe » en « écrire du code et le planifier ».

- [ ] Trait `Builder` + implémentation Go
- [ ] Node `Script` : source → dossier temporaire → `go mod init` → `go build` → artefact
- [ ] Cache par `source_hash` : source identique = pas de rebuild
- [ ] Build asynchrone, statut et logs de build consultables
- [ ] Protocole de données : **Arrow IPC** sur stdin/stdout, stderr journalisé. Pas du
      JSON par ligne — à 350 colonnes il répéterait 350 noms de champs par ligne, et le
      coût de sérialisation dépasserait le traitement lui-même
- [ ] Trancher **D7** avec la DSI : `GOPROXY` interne, ou build hors ligne et vendoring

**Concepts :** design par traits, `async_trait`, `sha2`, `tempfile`

**Critère d'acceptation :** deux enregistrements du même code ne déclenchent qu'un seul
build.

---

## ══ Jalon v0.5 — utilisable en production ══

À ce point, ntz remplace Task Scheduler. Ce qui existe : planification fiable avec
idempotence et reprise, jobs en commande ou en Go compilé à la volée, historique, logs
conservés, déclenchement manuel, authentification et audit.

Ce qui n'existe pas encore : le canvas, les nodes SQL/File/HTTP, le `Map`, l'alerting.

**À faire maintenant, pas plus tard :** migrer deux ou trois jobs réels et les laisser
tourner en parallèle des exe historiques. Tout ce qu'on apprendra là remettra en cause
des hypothèses de la suite — et il vaut mieux que ce soit avant d'avoir écrit l'éditeur
de `Map`.

---

## Étape 14 — Le temps réel

- [ ] SSE côté axum : événements de run et lignes de log
- [ ] Visionneuse de logs en flux, défilement automatique
- [ ] Reconnexion propre après coupure (`Last-Event-ID`)
- [ ] **Masquage des secrets dans le flux SSE**, pas seulement dans les logs stockés

**Concepts :** `axum::response::Sse`, `tokio_stream`, `EventSource`, invalidation
TanStack Query sur événement

**Critère d'acceptation :** lancer un run et suivre sa sortie sans rafraîchir. Couper le
réseau 10 s : à la reconnexion, aucun événement perdu, aucun doublon.

---

## Étape 15 — L'éditeur de nodes

Le morceau. `@xyflow/react` fait le canvas ; tout le reste est du modèle et de
l'ergonomie.

- [ ] `@xyflow/react` : canvas, minimap, contrôles, ajustement à la vue
- [ ] Store **Zustand** pour le graphe (`useState` ne tient pas la charge sur un canvas)
- [ ] Nodes personnalisés par `kind`, `Handle` par port
- [ ] Palette **groupée par famille**, alimentée par `GET /api/node-kinds`, glisser-déposer
- [ ] **Formulaire de configuration généré depuis le JSON Schema** du `kind`
- [ ] **Schéma de chaque port affiché sur le canvas** — sans ça, `Map` sera inutilisable
- [ ] Arêtes données / contrôle distinguées par le style **et** un libellé, jamais par la
      seule couleur
- [ ] Refus immédiat d'un branchement invalide : cycle (`getOutgoers`), schémas
      incompatibles, port d'entrée de données déjà occupé
- [ ] Erreurs de validation serveur reportées **sur les nodes**, pas dans une bannière
- [ ] Auto-sauvegarde du draft (debounce), bouton **Publier** explicite, annuler/rétablir
- [ ] Coloration des nodes en direct via le SSE de l'étape 14

**Concepts React :** stores externes et `useSyncExternalStore`, `nodeTypes` mémoïsé
**hors du composant** (le footgun classique : déclaré inline, il remonte tous les nodes à
chaque rendu), `React.memo` sur les nodes custom, Immer pour les mises à jour immuables

**Critère d'acceptation :** construire à la souris un job en losange avec une branche
d'erreur, le publier, le voir s'exécuter conformément à l'étape 7. Et sur un graphe de
200 nodes, déplacer un node reste fluide — si ça saccade, tous les nodes se re-rendent,
et c'est la leçon de perf React à intégrer.

---

# Les nodes de production

## Étape 16 — Contexte global : connexions, variables, secrets

Avant les nodes SQL, parce qu'un DSN ne doit jamais se retrouver dans un graphe.

- [ ] Tables `connection` et `variable`
- [ ] Chiffrement au repos : clé maîtresse via DPAPI ou fichier à ACL restreinte
- [ ] Test de connexion (`validated_at`), l'API ne renvoie **jamais** un secret déchiffré
- [ ] Masquage des secrets dans les logs **et** le SSE
- [ ] **Refus à la publication d'un DSN avec mot de passe posé en dur sur un node**

**Concepts :** `ring` ou `aes-gcm`, chiffrement enveloppe, `zeroize`, FFI DPAPI

**Critère d'acceptation :** un secret n'apparaît en clair ni dans la base, ni dans une
réponse d'API, ni dans un log, ni dans le flux SSE. Vérifié en cherchant activement,
pas en le supposant.

---

## Étape 17 — Nodes SQL

- [ ] Abstraction `trait SqlDialect` dès le départ — **deux clients** : `sqlx` pour
      PostgreSQL, `arrow-odbc` pour SQL Server (**D12** : `sqlx` a retiré MSSQL en 0.7,
      donc pas de macros vérifiées à la compilation côté SQL Server)
- [ ] `SQL Input` : **introspection du schéma** par préparation de la requête sans
      exécution ; **lecture par curseur** convertie en lots Arrow, mémoire constante ;
      paramètres **liés**, jamais interpolés
- [ ] `progress_column` pour la reprise incrémentale
- [ ] `SQL Output` : `Upsert` / `Truncate` / `Insert`, `key_columns` **obligatoire** pour
      l'upsert
- [ ] **Chargement en masse obligatoire, pas optionnel** : à 350 colonnes, la limite de
      paramètres plafonne un `INSERT` paramétré à **6 lignes** en SQL Server et 187 en
      PostgreSQL. Donc `COPY` binaire (`copy_in_raw`) alimenté depuis les tampons Arrow,
      et insertion en masse `arrow-odbc` côté SQL Server
- [ ] `ON CONFLICT` en PostgreSQL ; en SQL Server, table intermédiaire chargée en bulk
      puis `UPDATE` joint + `INSERT … WHERE NOT EXISTS`, plutôt que `MERGE`
- [ ] `commit_mode` `chunked` par défaut (**D18**), avec la perte d'atomicité **affichée**
- [ ] `Truncate` sur grosse table : chargement dans une table à côté puis **échange par
      renommage**, pour ne pas exposer une table vide pendant des heures

**Critère d'acceptation triple.** Fonctionnel : charger un million de lignes de
PostgreSQL vers SQL Server en `Upsert`, deux fois de suite, même état final. **Échelle :**
le débit en lignes/s ne s'effondre pas entre 1 M et 50 M de lignes, et la mémoire reste
plate. **Découplage :** cette étape n'a demandé **aucune modification du code de
l'éditeur**. Si elle en a demandé, le catalogue de nodes est mal découplé — le corriger
avant l'étape 18.

---

## Étape 18 — Nodes File et HTTP

- [ ] `File Input` / `File Output`, CSV et JSON, options de l'étape 4
- [ ] **Écriture atomique** : fichier temporaire puis renommage
- [ ] Liste blanche de racines autorisées (**D15**)
- [ ] `HTTP Request` unique (**H1**), modes `once` et `per_row`
- [ ] Limites de concurrence et de débit en `per_row` — 100 000 lignes = 100 000 appels
- [ ] Retry seulement sur timeout, 429, 5xx ; respecter `Retry-After`

**Critère d'acceptation :** un job qui lit un partage UNC, appelle une API ligne à ligne,
route les échecs vers un CSV de rejets et charge le reste en base. Tuer le service en
cours d'écriture ne laisse aucun fichier tronqué.

---

## Étape 19 — L'éditeur de `Map`

Une application à elle seule. Palier 1 uniquement (nodes.md §2) : 1 entrée → 1 sortie,
renommer, réordonner, supprimer, constante, conversion de type.

- [ ] **Auto-mapping par nom à la création** — exact, puis insensible à la casse, puis
      normalisé. À 350 colonnes c'est le comportement par défaut, pas une commodité :
      câbler 350 champs à la souris n'est pas une interface
- [ ] Compteur de non-mappés, et possibilité de ne travailler que sur eux
- [ ] Recherche et filtre sur les deux panneaux, opérations en masse
- [ ] Sous-éditeur deux panneaux pour les **exceptions**, câblage à la souris
- [ ] Types affichés, conversions invalides signalées à la conception
- [ ] Flux de rejet sur le port `error`
- [ ] Les expressions du palier 2 sont des **`Expr` DataFusion vectorisées**, et le
      langage exposé peut être du SQL — que les utilisateurs connaissent déjà (**D9** est
      remplacée par **D17** : Rhai par ligne aurait fait 315 milliards d'appels
      d'interpréteur)

**Critère d'acceptation :** mapper un CSV de 350 colonnes vers une table de 320 en
**moins de cinq minutes**, l'auto-mapping ayant fait l'essentiel et le travail manuel ne
portant que sur les écarts. Et voir les erreurs de type avant d'exécuter.

---

# Plateforme

## Étape 20 — Composition : un job comme node

- [ ] Node `Job`, ports dérivés de la signature de l'enfant
- [ ] **Version figée à l'insertion** (**D11**), action « mettre à jour » explicite
- [ ] Détection de cycle **inter-jobs** (`job_dependency`) + profondeur maximale
- [ ] Vérification de contrat à la publication, liste des jobs dépendants affichée
- [ ] `run.parent_run_id` : descendre dans les sous-runs depuis l'historique

**Critère d'acceptation :** deux jobs qui s'embarquent mutuellement sont refusés avec le
chemin du cycle. Republier un job enfant ne modifie aucun parent.

---

## Étape 21 — Télémétrie et alerting

- [ ] `/metrics` Prometheus, tableaux de bord Grafana
- [ ] Métriques par run et par node : durée, statut, `rows_in`/`rows_out`/`rows_error`
- [ ] Trait `Notifier` + mail (`lettre`) et Teams (webhook via `reqwest`)
- [ ] Déclencheurs : node en échec, **run manqué**, run `interrupted`, dépassement de
      durée, taux de rejet anormal
- [ ] Anti-spam : regroupement et étouffement

**Critère d'acceptation :** le cas « le run n'a pas démarré du tout » alerte — c'est
précisément ce que Task Scheduler ne signale jamais. Et une base coupée 30 minutes
produit une notification, pas quarante.

---

## Étape 22 — Durcissement Windows

Rust avancé : `unsafe` et FFI. À garder pour la fin.

- [ ] Windows Job Objects avec `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`
- [ ] Un timeout tue tout l'arbre de process, pas seulement l'enfant direct
- [ ] Limite mémoire par node
- [ ] **Métriques de ressources par node** (CPU, mémoire) — elles dépendent des Job
      Objects, d'où leur absence à l'étape 21
- [ ] Compte de service dédié à privilèges réduits

**Concepts :** crate `windows` ou `win32job`, `unsafe`, handles et RAII (`Drop`)

**Critère d'acceptation :** un node `Script` qui lance un sous-process ne laisse aucun
orphelin après un timeout. Vérifié au Gestionnaire des tâches, pas déduit.

---

## Étape 23 — Mise en production

- [ ] Service Windows (`windows-service`)
- [ ] Un seul exe : front embarqué, pas de Node.js en production
- [ ] Fin de la migration : bascule job par job, les exe historiques désactivés en dernier
- [ ] Sauvegarde de la base, purge des logs selon **D5**
- [ ] Procédure de restauration documentée **et testée** une fois

**Critère d'acceptation :** un job migré tourne 15 jours sans écart avec l'ancien exe
avant qu'on désactive ce dernier.

---

## Hors v1, assumé

Reporté explicitement, pour que ce soit un choix et non un oubli. Détail et
justifications dans [nodes.md](nodes.md) §9.

Boucle `Tant que` (casse le tri topologique — la sortie propre est un node conteneur) ·
`Map` paliers 2 à 4, mais nettement moins chers depuis **D17** — DataFusion fournit
expressions vectorisées, jointures et déversement disque · bouton IA sur
SQL Input (validation DSI : le schéma, jamais les lignes) · `Script` Python (second
runtime, décision distincte de Go) · Excel · SFTP · pagination HTTP · node `Union` ·
déclencheur webhook (**D13**).

---

## Suivi des décisions

Les dix-neuf décisions sont dans [architecture.md §8](architecture.md#8-décisions), chacune
avec une position par défaut et l'étape où la trancher. **D3** (changement d'heure),
**D8** (représentation du graphe), **D14** (erreurs ligne à ligne) et **D17** (DataFusion)
sont à trancher tôt, aux étapes 2, 3, 4 et 5.

**Quatre décisions ont été renversées ou remplacées**, et c'est écrit comme tel plutôt que
réécrit en douce :

| | Position abandonnée | Pourquoi |
|---|---|---|
| **D4** | pas de données entre les nodes | le flux de données est le cœur du produit |
| **D10** | matérialisé en v1, le flux plus tard | à 900 M × 350, le matérialisé plafonne à 1,4 M de lignes |
| **D17** | opérateurs écrits à la main | réécrire tris et jointures hors-mémoire n'est pas un exercice, c'est DataFusion en moins bien |
| **D9** | Rhai évalué par ligne | 315 milliards d'appels d'interpréteur ; les `Expr` DataFusion sont vectorisées |
