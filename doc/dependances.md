# Dépendances — état, coût, alternatives

**Vérifié le 2026-08-03** sur l'API crates.io et les sites éditeurs. Cet état se périme :
la date de dernière release et le volume de téléchargements récents en sont les meilleurs
indicateurs, à revérifier avant chaque étape qui engage une dépendance.

Lecture des colonnes :

- **Maintenance** — dernière version publiée, et téléchargements récents comme proxy
  d'adoption. Une lib sans release depuis > 18 mois est signalée même si elle fonctionne.
- **Prix** — coût de licence réel. « Gratuit » = licence permissive (MIT, Apache-2.0),
  usage commercial inclus.
- **Si on la perd** — l'alternative, ou l'ordre de grandeur pour la réécrire. Les estimations
  sont grossières et supposent quelqu'un qui apprend Rust en même temps : à lire comme des
  facteurs de risque, pas comme un devis.

---

## 1. Le verdict en une ligne

**Aucun poste payant obligatoire.** La totalité de la pile est sous licence permissive.
Le seul risque sérieux n'est pas financier, c'est **`tiberius`, sans release depuis
juillet 2024**, sur le chemin critique du support SQL Server — et il existe une meilleure
route (§3.2).

---

## 2. Socle

| Crate | Version | Maintenance | Prix | Si on la perd |
|---|---|---|---|---|
| `tokio` | — | référence de l'écosystème async | Gratuit | Aucune alternative crédible. Non substituable, et non risqué. |
| `serde` | — | omniprésent | Gratuit | Idem. |
| `clap` | — | actif | Gratuit | `argh`, `bpaf`. Ou parsing maison : 2–3 j. Risque nul. |
| `anyhow` / `thiserror` | — | actif (dtolnay) | Gratuit | `eyre`/`snafu`. Ou enums d'erreur à la main : quelques jours. Risque nul. |
| `chrono` + `chrono-tz` | — | actif | Gratuit | `time` + `jiff`. `jiff` est plus récent et mieux conçu sur les fuseaux — à considérer dès l'étape 2 plutôt que de migrer après. |
| `croner` | **3.0.1** (oct. 2025) | 2,17 M téléch. récents | Gratuit | `cron`, `saffron`. Réécrire un parseur cron correct : 3–5 j. **Mais la partie difficile (D3, changement d'heure) est de toute façon à nous**, la lib ne fait que le parsing. Risque faible. |
| `uuid`, `sha2`, `tempfile` | — | actifs | Gratuit | Non substituables en pratique, risque nul. |

---

## 3. Données — le cœur, et le seul vrai risque

### 3.1 Moteur

| Crate | Version | Maintenance | Prix | Si on la perd |
|---|---|---|---|---|
| `arrow` / `arrow-schema` | — | Apache, actif | Gratuit | **Non substituable.** Réécrire une représentation colonnaire : années. C'est le socle de `D10`. |
| `datafusion` | **54.1.0** (21 juil. 2026) | 4,38 M téléch. récents, releases mensuelles | Gratuit (Apache-2.0) | **Non substituable.** Réécrire tris/jointures/agrégations avec déversement disque : années, en moins bien. C'est tout l'objet de `D17`. |
| `sqlx` | **0.9.0** (mai 2026) | 32 M téléch. récents | Gratuit | `tokio-postgres` (sfackler, très mature) si on renonce aux macros vérifiées à la compilation. Migration : ~1 semaine. Risque faible. |

> **Le coût caché de DataFusion : la cadence des versions majeures.** 54.x en août 2026,
> avec des majeures toutes les 4 à 8 semaines et une API qui bouge. Ce n'est pas une raison
> de l'éviter, mais **c'est un poste de maintenance à budgéter** : prévoir une montée de
> version régulière plutôt qu'un saut de dix majeures dans deux ans.

> **Piège d'alignement de versions, à connaître avant de perdre une journée.** `datafusion`,
> `arrow-odbc` et tout crate exposant des `RecordBatch` épinglent chacun **leur** version
> d'`arrow`. Si deux d'entre eux ne pointent pas la même, le compilateur voit **deux types
> `RecordBatch` distincts** et refuse de les mélanger, avec un message peu parlant.
> Vérifier `cargo tree -d` dès qu'on ajoute un crate de l'écosystème Arrow.

### 3.2 Connecteurs bases — le point à trancher

| Option | Version | Maintenance | Prix | Verdict |
|---|---|---|---|---|
| `tiberius` (TDS natif) | 0.12.3 | **juillet 2024 — 2 ans sans release.** 595 k téléch. récents, dépôt `prisma/tiberius` | Gratuit | ⚠️ **Le risque principal du projet.** Fonctionne, largement utilisé, mais Prisma l'a manifestement désinvesti. |
| **`arrow-odbc` + `odbc-api`** | **25.3.0** (20 juil. 2026) / **29.0.0** | **Très actif** — releases mensuelles, mainteneur pacman82 | Gratuit | ✅ **La route recommandée.** |
| Sidecar .NET | — | — | Gratuit | Repli de dernier recours : un process qui fait le pont. Ajoute un runtime et une frontière réseau. |

**Pourquoi `arrow-odbc` plutôt que `tiberius`**, et ça dépasse la question de la maintenance :

- il lit et écrit **directement des `RecordBatch` Arrow** — c'est exactement l'interface dont
  ntz a besoin, alors que `tiberius` demanderait une couche de conversion ligne → colonne
  à écrire et à maintenir ;
- il apporte le **chargement en masse**, indispensable vu la limite de 2 100 paramètres de
  SQL Server (nodes.md §3) ;
- il ouvre **toutes les sources ODBC** d'un coup — Oracle, DB2, fichiers Access, l'AS/400
  d'un concessionnaire — pour un coût marginal nul. Dans un groupe de concessions, ça
  arrivera.

Le prix à payer : le **driver ODBC de Microsoft doit être installé sur le serveur** (gratuit,
mais c'est une dépendance système à gérer dans le déploiement), et ODBC ajoute une couche
FFI donc `unsafe` en dessous — `odbc-api` l'encapsule, mais le débogage est moins agréable
qu'en Rust pur.

**Coût de l'écrire soi-même :** un driver TDS, c'est des mois. Ce n'est pas une option, et
c'est précisément pourquoi la vétusté de `tiberius` compte.

> **Décision à inscrire (candidate D20) :** `sqlx` pour PostgreSQL — métadonnées ntz *et*
> bases cibles, avec les macros vérifiées à la compilation — et **`arrow-odbc` pour SQL
> Server et tout le reste**. `tiberius` écarté pour vétusté, pas pour défaut technique.
> Ça révise `D12`, qui le désignait.

---

## 4. Web et API

| Crate | Version | Maintenance | Prix | Si on la perd |
|---|---|---|---|---|
| `axum` + `tower` / `tower-http` | — | actif (équipe tokio) | Gratuit | `actix-web`, `poem`. Migration : ~1 semaine. Risque faible. |
| `utoipa` | **5.5.0** (mai 2026) | 12,16 M téléch. récents | Gratuit | `aide`, ou écrire l'OpenAPI à la main (2–3 j). Porte `D19`, donc à surveiller — mais l'adoption est massive. |
| `schemars` | **1.2.2** (27 juil. 2026) | **131 M téléch. récents** | Gratuit | Générer le JSON Schema à la main pour nos ~10 types de nodes : 1–2 semaines. Risque très faible. |
| `rust-embed` | **8.12.0** (juil. 2026) | 12,3 M téléch. récents | Gratuit | `include_dir`, ou un `build.rs` maison : 1 j. Risque nul. |
| `reqwest` | — | actif | Gratuit | `hyper` directement, ou `ureq`. Risque faible. |
| `tokio-stream` | — | actif | Gratuit | Non risqué. |

---

## 5. Windows

| Crate | Version | Maintenance | Prix | Si on la perd |
|---|---|---|---|---|
| `windows-service` | **0.8.1** (8 mai 2026) | 1,53 M téléch. récents, maintenu par **Mullvad** | Gratuit | Réécrire via le crate `windows` : ~1 semaine (handler de contrôle, machine à états). Risque faible. |
| `win32job` | **2.0.3** (mai 2025) | 407 k téléch. récents | Gratuit | C'est un mince wrapper sûr autour des Job Objects — le refaire via `windows` : **2–3 j**. Risque quasi nul même s'il est abandonné. |
| `windows` | — | **Microsoft** | Gratuit | Non substituable, et non risqué. |
| Chiffrement (`ring` ou `aes-gcm`, `zeroize`) | — | actifs | Gratuit | Ne **jamais** écrire soi-même. Si `ring` pose problème, `aws-lc-rs` ou `aes-gcm` (RustCrypto). |

---

## 6. Front

| Lib | Maintenance | Prix | Si on la perd |
|---|---|---|---|
| **React Flow** (`@xyflow/react`) | actif, société xyflow | **Cœur gratuit, MIT, usage commercial inclus.** Un abonnement **Pro** optionnel (Starter / Professional / Enterprise) donne des exemples avancés, des templates, des issues GitHub prioritaires et du support — **aucune fonctionnalité du cœur n'est bridée** | Réécrire le canvas : **4–8 semaines** + maintenance perpétuelle (pan/zoom, hit-testing, courbes, minimap, sélection, culling). Cf. `D19`. |
| React 19 + Vite + TypeScript | actifs | Gratuit | Non substituables en pratique. |
| TanStack Query | actif | Gratuit | Réécrire cache + invalidation + états : plusieurs semaines, et mal. Ne pas essayer. |
| Zustand | actif | Gratuit | ~1 j — la lib est minuscule. Risque nul. |
| Immer | actif | Gratuit | Écrire les mises à jour immuables à la main : gratuit, juste plus verbeux. Risque nul. |
| **CodeMirror 6** ou Monaco | actifs | Gratuit (MIT) | **Non substituable.** C'est l'argument qui a écarté egui dans `D19`. |
| Rendu de formulaires depuis JSON Schema (`rjsf`, `JSONForms`) | actifs | Gratuit (Apache-2.0 / MIT) | ⚠️ **Vrai point de décision.** Le rendu générique de ces libs est souvent laid et pénible à styler ; beaucoup d'équipes finissent par écrire le leur. Pour notre sous-ensemble de types : **2–3 semaines**. À évaluer sur un prototype à l'étape 15, pas à décider maintenant. |
| Tailwind + shadcn/ui | actifs | Gratuit (shadcn = copié dans le projet) | Tailwind Plus est payant et **inutile ici**. |
| Vitest + React Testing Library | actifs | Gratuit | Non risqués. |
| `openapi-typescript` / `orval` / `hey-api` | actifs | Gratuit | Écrire le générateur pour notre OpenAPI : ~1 semaine. Risque faible. |

> **À éviter : les grilles de données commerciales.** AG Grid et équivalents facturent
> l'édition entreprise. Nos besoins de tableau (historique des runs, éditeur de `Map`) sont
> couverts par du HTML plus une virtualisation (`@tanstack/react-virtual`, gratuit).

---

## 7. Infrastructure

| Élément | Prix | Note |
|---|---|---|
| PostgreSQL | Gratuit | Métadonnées ntz. |
| Prometheus + Grafana | Gratuit en auto-hébergé | Grafana Cloud est payant et non nécessaire. Grafana OSS est en AGPL — sans conséquence ici puisqu'on ne le redistribue pas, on le déploie à côté. |
| Driver ODBC Microsoft | Gratuit | À installer sur le serveur (§3.2). |
| SQL Server | Déjà licencié | C'est une base **cible**, pas un achat nouveau. |
| Toolchain Go | Gratuit | Pour le node `Script` (étape 13). |
| Docker | Gratuit en usage dev | Docker **Desktop** est payant au-delà d'un seuil d'effectif en entreprise — à vérifier avec la DSI, ou utiliser Podman / une VM. Seul poste où une facture peut apparaître par surprise. |

---

## 8. Risques classés

| # | Risque | Gravité | Réponse |
|---|---|---|---|
| 1 | **`tiberius` sans release depuis juillet 2024** | Élevée — chemin critique | Basculer sur `arrow-odbc` (§3.2). Ce n'est pas un contournement, c'est un meilleur choix technique. |
| 2 | **Cadence des majeures DataFusion** | Moyenne, permanente | Monter de version régulièrement. Isoler DataFusion derrière notre trait `Node` pour que la casse d'API reste locale. |
| 3 | **Rendu de formulaires JSON Schema** | Moyenne | Prototyper à l'étape 15 avant de s'engager. Budgéter 2–3 semaines si on l'écrit. |
| 4 | Alignement des versions `arrow` entre crates | Faible mais agaçante | `cargo tree -d` en réflexe. |
| 5 | Docker Desktop en entreprise | Faible, mais budgétaire | Vérifier avec la DSI. |
| 6 | `win32job` peu actif | Très faible | 2–3 j pour s'en passer. |

---

## 9. Hygiène à mettre en place tôt

Pas à la fin — ces outils ne servent que s'ils tournent depuis le début.

- [ ] **`cargo-deny`** en CI : licences autorisées, doublons de versions, avis de sécurité.
      C'est lui qui attrape une dépendance en GPL avant qu'elle soit dans le binaire livré.
- [ ] **`cargo-audit`** (RustSec) sur les vulnérabilités connues.
- [ ] **`Cargo.lock` committé** — c'est un binaire d'application, pas une bibliothèque.
- [ ] **Renovate ou Dependabot**, en regroupant les mises à jour mineures pour ne pas noyer
      la revue.
- [ ] **`npm audit` + lockfile committé** côté front, et `npm ci` en CI, jamais `npm install`.
- [ ] Un inventaire des licences généré à la construction et **archivé avec la release** —
      utile le jour où la DSI le demandera.

---

## 10. Ce que je n'ai pas vérifié

Par honnêteté sur la fiabilité de ce document :

- **Les tarifs exacts de React Flow Pro** — la page de prix a changé d'URL et renvoyait un
  404 le 2026-08-03. Ce qui est confirmé : le cœur est MIT, gratuit, usage commercial inclus,
  et aucune fonctionnalité n'est bridée. Les montants restent à lire sur leur site si vous
  envisagez l'abonnement pour le support.
- **Les seuils de licence Docker Desktop** — à confirmer avec la DSI.
- Les crates marqués « — » en version sont des dépendances de l'écosystème dont la santé
  n'est pas en question ; je n'ai pas relevé leurs numéros un par un.
