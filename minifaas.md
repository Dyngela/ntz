# Mini-FaaS en Rust — Document d'architecture

> Document de référence pour construire un « AWS Lambda maison » simplifié.
> Objectif : **apprendre Rust en le codant soi-même**. Ce doc donne la carte
> (composants, contrats, schémas, ordre de construction) — pas les corps de
> fonctions. À toi d'écrire l'implémentation.

---

## 1. But et principe directeur

Une plateforme où l'on enregistre une **fonction** (du code Go, plus tard Python),
et où cette fonction se déclenche de trois façons : **planning (cron)**,
**appel API**, ou **webhook entrant**. La fonction s'exécute dans un **bac à sable
WASM** embarqué dans le binaire Rust.

Le principe qui structure tout le projet :

> **Rust est le plan de contrôle. WASM est le bac à sable. La fonction n'est
> qu'une charge utile.**

Rust possède les permissions réelles (réseau, disque, secrets). La fonction, elle,
ne peut rien faire d'autre que ce que l'hôte Rust lui autorise explicitement. C'est
exactement le modèle d'un vrai FaaS.

Non-objectifs (pour rester concentré) : pas de multi-tenant, pas d'authentification
fine, pas de scaling horizontal, pas de haute disponibilité. Un seul binaire, une
seule machine, une base locale.

---

## 2. Vue d'ensemble

```
Déclencheurs            Plan de contrôle (Rust)          Exécution
─────────────           ───────────────────────          ─────────
Scheduler (cron) ─┐
Appel API        ─┼──►  Dispatcher ──► Runtime ──►  Bac à sable WASM (wasmtime)
Webhook entrant  ─┘         │             │              │
                            ▼             ▼              ▼
                          Store       Artefact       Logs + résultat
                        (SQLite)      (.wasm)          (capturés)
```

Cinq responsabilités, à garder bien séparées :

1. **API** — reçoit les requêtes HTTP (gestion des fonctions, invocations, webhooks).
2. **Store** — persiste les fonctions, les déclencheurs, l'historique d'exécution.
3. **Scheduler** — boucle durable qui déclenche les jobs cron même après un crash.
4. **Runtime** — abstrait « comment on exécute une fonction » (contrat commun).
5. **Bac à sable WASM** — instancie et fait tourner le module, capture la sortie,
   applique les limites.

---

## 3. Structure du projet

Commence en **binaire unique avec des modules** ; tu extrairas en workspace plus
tard si le besoin s'en fait sentir. Découpage suggéré :

```
src/
  main.rs         Point d'entrée : charge la config, ouvre le store, lance API + scheduler
  config.rs       Paramètres (port, chemin DB, limites par défaut)
  api/            Serveur axum : routes, handlers, désérialisation des requêtes
  store/          Accès base : fonctions, triggers, runs (le seul module qui parle SQL)
  scheduler/      Boucle de réconciliation cron
  dispatcher.rs   Choisit le runtime, réserve un run, orchestre l'exécution
  runtime/        Le trait Runtime + l'implémentation WasmRuntime
  wasmhost/       Setup wasmtime : Engine, Linker, host functions, pont mémoire
  sdk-go/         (dossier Go séparé) Le package importé par les fonctions Go
```

Règle d'or : **un seul module touche la base** (`store`). Tout le reste passe par
lui. Ça t'évitera de disperser du SQL partout et t'apprendra à concevoir une
frontière propre.

---

## 4. Modèle de données

Trois tables. Le schéma est volontairement minimal — tu l'enrichiras.

```sql
-- Une fonction = du code + le langage + l'artefact compilé
CREATE TABLE functions (
    id           TEXT PRIMARY KEY,        -- uuid
    name         TEXT NOT NULL,
    language     TEXT NOT NULL,           -- 'go' | 'python' (plus tard)
    source       TEXT NOT NULL,           -- code source brut
    wasm_path    TEXT,                    -- chemin de l'artefact .wasm compilé (NULL tant que pas buildé)
    version      INTEGER NOT NULL DEFAULT 1,
    created_at   TEXT NOT NULL,
    updated_at   TEXT NOT NULL
);

-- Un déclencheur relie une fonction à une source d'événement
CREATE TABLE triggers (
    id             TEXT PRIMARY KEY,
    function_id    TEXT NOT NULL REFERENCES functions(id),
    kind           TEXT NOT NULL,         -- 'schedule' | 'api' | 'webhook'
    config         TEXT NOT NULL,         -- JSON : { "cron": "0 9 * * *" } ou { "path": "/hooks/xyz" }
    enabled        INTEGER NOT NULL DEFAULT 1,
    -- État de planification (seulement pour kind='schedule') :
    next_run_at    TEXT,                  -- clé de la durabilité (voir §8)
    last_run_at    TEXT,
    catch_up       TEXT NOT NULL DEFAULT 'coalesce' -- 'coalesce' | 'backfill' | 'skip'
);

-- Un run = une exécution concrète, avec sa déduplication
CREATE TABLE runs (
    id             TEXT PRIMARY KEY,
    function_id    TEXT NOT NULL,
    trigger_id     TEXT,
    scheduled_for  TEXT NOT NULL,         -- créneau visé (pour cron) ou instant de l'appel
    status         TEXT NOT NULL,         -- 'pending' | 'running' | 'success' | 'failed' | 'timeout'
    started_at     TEXT,
    finished_at    TEXT,
    stdout         TEXT,
    stderr         TEXT,
    error          TEXT,
    leased_until   TEXT,                  -- pour détecter les runs abandonnés après crash
    UNIQUE(function_id, scheduled_for)    -- ← empêche le double déclenchement d'un même créneau
);
```

La contrainte `UNIQUE(function_id, scheduled_for)` est la pièce maîtresse de
l'idempotence : c'est elle qui garantit qu'un créneau cron ne s'exécute qu'une
fois, même après un redémarrage. Voir §8.

---

## 5. Le modèle d'exécution WASM — chapitre central

C'est le cœur du projet et ce qui t'apprendra le plus. Prends le temps de bien
comprendre cette section avant de coder l'exécuteur.

### 5.1 Deux phases distinctes

Ne les confonds jamais :

- **Build-time (une fois par version de fonction)** : compiler le code Go en un
  module WASM autonome avec **TinyGo**, ciblant WASI. Tu stockes le `.wasm`
  produit. C'est un appel à un outil externe (`tinygo build ...`) que ton Rust
  lance en sous-processus, puis dont il récupère le fichier.

- **Run-time (à chaque déclenchement)** : charger ce `.wasm` dans **wasmtime**,
  l'instancier, lui passer l'entrée, récupérer la sortie, appliquer les limites.

### 5.2 Les objets wasmtime à comprendre

Cinq concepts, dans l'ordre où ils s'enchaînent :

- **`Engine`** — compile et met en cache le bytecode WASM. Coûteux à créer :
  **un seul pour toute l'appli**, partagé.
- **`Module`** — un `.wasm` compilé par l'Engine. Réutilisable entre invocations
  d'une même fonction ; mets-le en cache par (function_id, version).
- **`Store`** — l'état d'**une** instance : la mémoire, le contexte WASI, et tes
  données hôte. **Un Store neuf par invocation** (c'est ça qui garantit l'isolation
  entre deux exécutions).
- **`Linker`** — câble les imports du module : le contexte WASI et **tes host
  functions** (les utilitaires Rust exposés au Go).
- **`Instance`** — le module instancié dans un Store, prêt à tourner.

Cycle mental d'une invocation :

```
Module (caché) + Store (neuf) + Linker (WASI + host funcs)
      └────────────► Instance ────► exécution ────► lecture sortie ────► drop Store
```

### 5.3 Convention d'entrée / sortie

Le plus simple, et celui que je te recommande pour démarrer : **WASI +
stdin/stdout**. L'hôte écrit l'entrée (JSON) sur le stdin virtuel de l'instance ;
la fonction lit stdin, fait son travail, écrit le résultat sur stdout ; l'hôte lit
stdout à la fin.

Avantages : côté Go tu écris du `main()` tout à fait normal (`fmt.Println`,
lecture de `os.Stdin`), et ça reprend exactement le modèle du sous-processus, donc
la migration mentale est nulle. Tu compliqueras plus tard avec une fonction
exportée + passage par pointeurs si tu veux du typage plus fin.

```
Hôte Rust                          Fonction Go (dans le WASM)
─────────                          ──────────────────────────
écrit input JSON ── stdin ───►     lit os.Stdin, parse
                                   traite
lit stdout    ◄── stdout ───       écrit résultat JSON sur stdout
```

### 5.4 Isolation et limites — la partie sécurité

Le sandbox WASM ne vaut que par ce que tu **refuses**. Par défaut, WASI ne donne
**aucun** accès disque ni réseau — c'est le comportement voulu, ne l'ouvre pas sans
raison. Quatre garde-fous à mettre en place :

- **CPU : fuel metering.** wasmtime peut compter le « carburant » consommé par le
  module et l'arrêter au-delà d'un seuil. Empêche les boucles infinies.
- **Timeout mural : epoch interruption.** Un mécanisme d'interruption périodique
  qui coupe une instance qui dépasse un délai réel (utile si le code bloque sans
  brûler de fuel, ex. attente).
- **Mémoire : `StoreLimits`.** Plafonne la mémoire linéaire que l'instance peut
  allouer.
- **Capacités : deny by default.** N'accorde un accès (une variable
  d'environnement, un fichier, une host function) que si la fonction en a besoin.

Chacune de ces limites est un excellent petit exercice Rust isolé.

### 5.5 Host functions — donner des utilitaires Rust au Go

C'est ce dont on a parlé : exposer du Rust à la fonction. Le principe :

1. Tu enregistres une fonction sur le **`Linker`** (ex. `env.kv_get`).
2. Le module Go l'**importe** et l'appelle (`//go:wasmimport env kv_get`).
3. Au moment de l'appel, ta fonction Rust reçoit un **`Caller`** qui donne accès à
   la mémoire linéaire de l'instance.

**Le piège du passage de chaînes.** La frontière WASM ne transporte que des
nombres (i32/i64/f32/f64). Pour passer une chaîne, le Go passe un **pointeur + une
longueur** dans sa mémoire linéaire ; côté Rust, tu lis ces octets **dans la
mémoire de l'instance** via le `Caller`. Pour renvoyer une chaîne, c'est l'inverse
et c'est plus délicat (il faut que le module alloue un tampon). Schéma :

```
Go : kv_get(ptr, len)  ──►  Rust lit [ptr..ptr+len] dans la mémoire de l'instance
                            fait le vrai travail (accès au store, etc.)
                       ◄──  Rust écrit le résultat dans la mémoire de l'instance
```

Le **Component Model + WIT** automatisent ce marshalling — mais c'est un terrain
qui bouge vite. Commence à la main sur une ou deux host functions simples pour
comprendre le mécanisme, puis évalue le Component Model comme évolution.

> ⚠️ Écosystème mouvant : TinyGo/WASI, wasmtime host bindings et le Component Model
> évoluent de version en version. Au moment de coder cette partie, vérifie l'état
> courant de l'API wasmtime et des cibles TinyGo — l'architecture ci-dessus reste
> valable, les détails d'API non.

---

## 6. Le contrat `Runtime`

L'abstraction qui rend ton moteur multi-langage et multi-mode d'exécution. Tu
implémenteras `WasmRuntime` ; plus tard un `PythonRuntime` (interpréteur embarqué)
suivra le même contrat. **Signatures à implémenter toi-même** — ceci est le
contrat, pas le code :

```rust
trait Runtime {
    fn language(&self) -> Language;

    // Build-time : source → artefact .wasm (lance TinyGo en sous-processus)
    async fn build(&self, source: &str) -> Result<Artifact>;

    // Run-time : instancie, exécute, capture
    async fn run(&self, artifact: &Artifact, input: Invocation) -> Result<Outcome>;
}
```

Types associés (esquisse des champs, à toi de les définir précisément) :

- `Artifact` — chemin (ou octets) du `.wasm` compilé.
- `Invocation` — la charge d'entrée (JSON), le contexte du déclencheur, les limites
  effectives (fuel, mémoire, timeout).
- `Outcome` — statut, stdout, stderr, éventuelle erreur, durée.
- `Language` — enum (`Go`, plus tard `Python`).

Le `dispatcher` choisit l'implémentation selon `function.language`, via
`Box<dyn Runtime>` (dispatch dynamique) ou un enum (dispatch statique) — bon
moment pour comparer les deux approches en Rust.

---

## 7. Les déclencheurs — flux

### 7.1 Appel API (synchrone)

```
POST /functions/:id/invoke  { input }
  → charge la fonction depuis le store
  → dispatcher.run(function, input, trigger=api)
  → renvoie l'Outcome dans la réponse HTTP
```

### 7.2 Webhook (entrant)

```
POST /hooks/:path  { corps arbitraire }
  → résout le trigger dont config.path == :path
  → dispatcher.run(function, corps, trigger=webhook)
  → répond (sync) ou accuse réception et exécute en tâche de fond (async)
```

### 7.3 Planning (cron)

Géré par le scheduler (§8), pas par l'API. La boucle repère les jobs dus et appelle
le dispatcher.

Dans les trois cas, l'orchestration converge vers le **même point** :
`dispatcher.run(...)`. Garde ce goulot unique — c'est lui qui réserve un `run`,
choisit le runtime, exécute, et enregistre le résultat.

---

## 8. Le scheduler durable

Rappel du problème : un timer en mémoire perd les créneaux pendant que le service
est mort. **Solution : la base est la source de vérité, pas le timer.**

Chaque trigger cron porte un `next_run_at`. Une boucle réconcilie régulièrement :

```
boucle toutes les 30 s :
    now = maintenant()
    dus = SELECT * FROM triggers
          WHERE kind='schedule' AND enabled=1 AND next_run_at <= now

    pour chaque trigger dû :
        # 1. réserver le créneau (idempotence)
        essayer INSERT INTO runs(function_id, scheduled_for=next_run_at, status='running', leased_until=now+Xmin)
        si l'insert échoue (UNIQUE violé) : quelqu'un l'a déjà pris → passer

        # 2. exécuter
        outcome = dispatcher.run(...)

        # 3. enregistrer + planifier le prochain créneau
        UPDATE runs   SET status=..., stdout=..., finished_at=now
        prochain = cron.next_after(now)      # crate croner
        UPDATE triggers SET last_run_at=now, next_run_at=prochain
```

Points de vigilance :

- **Réconciliation au démarrage** : au lancement, un trigger dont `next_run_at` est
  déjà passé sera vu au premier tour de boucle et rattrapé. C'est ça qui règle le
  cas « service mort de 8h59 à 9h01 ».
- **Politique de rattrapage** (`catch_up`) :
    - `coalesce` (défaut) — un seul rattrapage, puis on saute au prochain créneau
      futur. `next_after(now)` le fait naturellement.
    - `backfill` — rejouer chaque créneau manqué (jobs où chaque occurrence compte).
    - `skip` — au-delà d'un délai de tolérance, on marque raté sans exécuter.
- **Crash pendant l'exécution** : un run resté `running` avec `leased_until` dépassé
  est considéré abandonné ; un balayage périodique le repasse en `pending` pour
  reprise.
- **Précision** : un job « de 9h00 » part entre 9h00 et 9h00+intervalle_de_boucle.
  30 s suffisent largement pour du cron.

---

## 9. Sécurité — check-list

À cocher au fur et à mesure. Exécuter du code inconnu **exige** ces garde-fous :

- [ ] WASI en deny-by-default (pas de FS, pas de réseau accordés par défaut).
- [ ] Fuel metering activé (limite CPU).
- [ ] Epoch interruption activée (timeout mural).
- [ ] `StoreLimits` (plafond mémoire).
- [ ] Store neuf par invocation (pas de fuite d'état entre exécutions).
- [ ] Taille du code source et de l'artefact plafonnée.
- [ ] Host functions minimales et validant leurs entrées (une host function est une
  porte que tu ouvres — traite ses arguments comme hostiles).
- [ ] Sorties (stdout/stderr) tronquées à une taille max avant stockage.

---

## 10. Ordre de construction (jalons)

Chaque jalon produit quelque chose qui **marche** et enseigne un pan de Rust.
Ne saute pas d'étape.

- [ ] **M0 — Squelette.** Serveur axum (route `/health`), store SQLite ouvert,
  endpoints créer/lister une fonction (source seulement, aucune exécution).
  *Apprends : axum, sqlx, sérialisation, structure de projet.*

- [ ] **M1 — Exécution WASM d'un `.wasm` pré-compilé.** Tu compiles un Go en
  `.wasm` **à la main** avec TinyGo, tu l'uploades, et Rust l'instancie via
  wasmtime + WASI, passe une entrée sur stdin, capture stdout. **Le jalon clé.**
  *Apprends : wasmtime (Engine/Module/Store/Linker/Instance), WASI.*

- [ ] **M2 — Pipeline de build.** Rust lance TinyGo en sous-processus pour compiler
  la source en `.wasm`, stocke l'artefact, gère les erreurs de compilation.
  *Apprends : `tokio::process`, gestion d'erreurs, I/O fichiers.*

- [ ] **M3 — Déclencheurs API + webhook.** Endpoints d'invocation synchrone.
  *Apprends : routing dynamique, extraction de corps, état partagé.*

- [ ] **M4 — Scheduler durable.** `next_run_at`, boucle de réconciliation, table
  `runs`, politique `coalesce`. *Apprends : persistance, idempotence, transactions.*

- [ ] **M5 — Limites.** Fuel, mémoire, epoch/timeout. *Apprends : l'API de contrôle
  de wasmtime, la robustesse.*

- [ ] **M6 — Host functions + SDK Go.** Expose 1–2 utilitaires Rust (ex. un
  kv-store, un log structuré) via le Linker, et le package Go qui les enveloppe.
  *Apprends : le pont mémoire WASM, la conception d'API.*

- [ ] **M7 — Avancé (optionnel).** Pooling d'instances, runtime Python
  (interpréteur embarqué), Component Model / WIT.

Conseil : arrête-toi mentalement après **M3** pour un premier « ça marche pour de
vrai », après **M6** pour un projet dont tu es fier.

---

## 11. Crates et références à lire

**Crates Rust :**

- `axum` + `tokio` — serveur HTTP async.
- `serde` / `serde_json` — (dé)sérialisation.
- `sqlx` (SQLite) — store (alternative : `redb`, pur Rust).
- `wasmtime` + `wasmtime-wasi` — runtime WASM et contexte WASI.
- `croner` — parsing cron et calcul du prochain créneau.
- `thiserror` (erreurs de bibliothèque) / `anyhow` (erreurs d'application).
- `tracing` — logs structurés.
- `uuid` — identifiants.

**Outils externes :** TinyGo (compilation Go → WASM/WASI).

**À lire pour apprendre (cherche la version courante) :**

- Le *Rust Book*, chapitres sur les traits et la gestion d'erreurs.
- La doc d'embarquement de **wasmtime** (guide « embedding » côté Rust).
- La doc **WASI** (le modèle de capacités).
- La doc **TinyGo** pour la cible WASM/WASI et `//go:wasmimport`.

---

## 12. Décisions ouvertes et pièges connus

À trancher quand tu y arriveras — les noter t'évite de coder dans le flou :

- **stdin/stdout vs fonction exportée** : commence en stdin/stdout (§5.3), migre si
  tu veux du typage fin.
- **Instance par invocation vs pooling** : par invocation d'abord (sûr et simple),
  pooling en M7 (rapide mais nécessite de réinitialiser l'état).
- **Sync vs async pour les webhooks** : sync au début (plus simple à raisonner).
- **Renvoyer une chaîne depuis une host function** : le point le plus délicat du
  pont mémoire ; commence par des host functions qui ne renvoient qu'un nombre ou
  un code de statut.
- **Python plus tard** : ce n'est pas « compiler la fonction » mais « embarquer un
  interpréteur » (CPython-WASI, RustPython, componentize-py). Chapitre séparé, même
  contrat `Runtime`.

---

*Fin du document. Reviens-y à chaque jalon : la §5 (WASM) et la §8 (scheduler) sont
les deux endroits où l'on se perd le plus facilement.*