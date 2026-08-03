# ntz

Plateforme de jobs planifiés du Groupe Kroely. Remplace ~40 exécutables Windows lancés
1×/jour sans historique, sans logs conservés, sans alerte.

**Deux modes, un seul moteur.** `script` : du code Go ou une commande + un planning, graphe
`Start → Command|Script → End` synthétisé et jamais montré — c'est le **jalon v0.5** et la
priorité métier. `graph` : un DAG de nodes entre lesquels circule de la donnée, un **ETL
visuel** avec éditeur de nodes.

**La contrainte qui dimensionne tout : jusqu'à 900 M de lignes × 350 champs** (~315
milliards de cellules, ~6 To). Elle a déjà invalidé deux conceptions. Toute proposition doit
tenir à cette échelle avant d'être présentée.

Rust · PostgreSQL (métadonnées) · Arrow + DataFusion · React · Windows Server.

---

## Comment on travaille

**L'utilisateur écrit le code. Je conçois, je relis, je pointe vers ce qu'il faut lire.**
C'est la règle numéro un ; ntz est autant un projet d'apprentissage de Rust qu'un outil de
production.

- Poser le problème, nommer les concepts Rust en jeu, indiquer les crates de référence et
  les critères d'acceptation. Puis laisser coder.
- Fournir au besoin des **signatures, `trait`, squelettes de types** — pas des corps de
  fonctions. Pas d'implémentation complète non sollicitée.
- Relire son code, expliquer les erreurs du borrow checker, proposer des refactorings.
- **Pointer explicitement ce qui vaut d'être lu** : doc, source d'un crate, exemples.
  Lire du Rust compte autant qu'en écrire.
- **Ne jamais justifier une implémentation maison par « c'est formateur ».** Apprendre une
  bibliothèque de référence *est* l'apprentissage utile. Réécrire `petgraph`, Arrow ou
  DataFusion est un mauvais conseil, pas un exercice.

---

## Invariants

Ces règles ne se contournent pas. Elles se **modifient**, explicitement, en amendant la
décision concernée dans `doc/architecture.md` §8 avec une date et un motif. Ce qui est
interdit, c'est la dérive silencieuse — pas la révision argumentée. Quatre décisions ont
déjà été renversées ainsi, et c'était juste à chaque fois.

### Données

- **Arrow colonnaire, en flux de lots.** Jamais orienté ligne, jamais matérialisé en
  mémoire : à 350 colonnes, une représentation par ligne plafonne à ~1,4 M de lignes.
- **`Decimal128` pour tout montant, jamais `Float64`.** La donnée est financière ; une
  erreur d'arrondi sur une marge ne se découvre pas vite.
- **Arrow traverse les couches nu.** Ne jamais envelopper `RecordBatch` / `SchemaRef` dans
  un type ntz — ça casserait l'intérêt d'`arrow-odbc` et de DataFusion.
- **La mémoire de pointe ne dépend pas du volume.** C'est un critère de test, pas un espoir :
  le même job sur 50 M de lignes consomme ce qu'il consomme sur 50 000.
- `cargo tree -d` en réflexe : deux versions d'`arrow` dans l'arbre donnent deux types
  `RecordBatch` incompatibles, avec un message illisible.

### Moteur

- **DataFusion assure le plan de données. ntz assure le plan de contrôle.** Arêtes de
  contrôle, `join_policy`, retry, baux, points de reprise, ordonnancement : à nous.
- **Le graphe compile en *plusieurs* plans, découpés aux frontières d'effet de bord.** Un
  plan suppose ses opérateurs purs et rejouables ; `Command`, `Script`, `HTTP per_row` et
  tous les Output sont des **bornes de plan**, jamais des opérateurs dedans.
- Un node décrit une **intention** (`Relational(op)` | `SideEffect`), il ne s'exécute pas
  lui-même — sinon DataFusion ne peut plus fusionner une chaîne ni descendre les prédicats.

### Découpage

- **`ntz-domain` et `ntz-ports` ne dépendent jamais de `datafusion`, `sqlx`, `arrow-odbc`,
  `axum` ni `windows`.** C'est le crate qui fait la contrainte, pas le trait.
- **`datafusion` n'apparaît que dans `ntz-engine`.** C'est ce qui rend ses majeures
  fréquentes supportables.
- **Ne pas abstraire** : Arrow, la base de métadonnées, `axum`, `serde`, le scheduler.
- Avant d'écrire un trait : **« quelle est la deuxième implémentation ? »** Si la réponse est
  « un mock pour les tests », c'est généralement une mauvaise raison.

### Bases de données

- **`sqlx` pour PostgreSQL** (macros vérifiées à la compilation), **`arrow-odbc` pour SQL
  Server et tout le reste**. Pas `tiberius` (sans release depuis juillet 2024).
- **Chargement en masse obligatoire** : `COPY` binaire, insertion en masse ODBC. Un `INSERT`
  paramétré plafonne à **6 lignes** par ordre à 350 colonnes en SQL Server.
- **Paramètres liés, jamais interpolés** dans une requête.
- **Une transaction par node au maximum, jamais par job.** Pas de transaction distribuée.
- `commit_mode = chunked` par défaut, et la perte d'atomicité est **affichée**, pas cachée.
- `Truncate` sur grosse table : table intermédiaire puis renommage, jamais vider en place.

### Fiabilité

- **Garantie *au moins une fois*, pas *exactement une fois*.** Aux jobs d'être rejouables ;
  c'est le sens du mode d'un SQL Output, à présenter comme tel dans l'UI.
- Le **`run`** est l'unité de réclamation et de retry, pas le `node_run` — les lots vivent en
  mémoire, donc un run tient dans un processus.
- Points de reprise **conditionnels** : colonne monotone + mode `Upsert` + aucun node
  bloquant. Si une condition manque, **l'API le dit** au lieu de le laisser découvrir le jour
  de l'incident.
- Un job de trois heures qui échoue à 95 % ne recommence pas au début.

### Sécurité

- **Jamais de DSN avec mot de passe dans un graphe** — il finirait dans le draft, chaque
  version, l'historique et l'UI. Un node **référence** une connexion.
- Masquage des secrets dans les logs **et dans le flux SSE**.
- `SecretStore::unseal` rend du `Zeroizing`. L'API ne renvoie jamais un secret déchiffré.
- Accès fichiers restreints à une **liste blanche de racines**.
- **Jamais de données client réelles** dans une fixture de test, un exemple, ou un prompt.
  Générer des jeux de données synthétiques. Règle Groupe Kroely, et bon sens.

### Front

- **React, avec les types générés depuis Rust** via l'OpenAPI. Aucun type d'API écrit à la
  main. La CI échoue si régénérer produit un diff.
- **Ne pas typer statiquement les configs de nodes** : `unknown` + JSON Schema servi par
  l'API. Les typer recoupleraient le front au catalogue, et ajouter un node doit coûter zéro
  ligne de React.
- **Aucun `useEffect` pour aller chercher des données.** TanStack Query.
- `nodeTypes` mémoïsé **hors du composant**, sinon tous les nodes se remontent à chaque rendu.
- Muter un état ne redessine rien — immutabilité obligatoire (Immer sur le graphe).
- Arêtes données / contrôle distinguées par le style **et** un libellé, jamais par la seule
  couleur.

---

## Bonnes pratiques

### Rust

- `anyhow` dans les binaires, `thiserror` dans les libs. **Aucun `unwrap()` ni `expect()`**
  hors tests.
- **Les erreurs nomment la donnée fautive** — la colonne, le port, le node, les nodes du
  cycle. Jamais « invalid graph ».
- **`Clock` injecté, jamais `now()` dans un test.** Trois lignes de trait, et c'est ce qui
  rend testable la politique de changement d'heure.
- **Pas d'`unsafe` hors `ntz-runtime`** (FFI Windows), et encapsulé derrière un type RAII.
- `async_trait` pour les traits appelés en `dyn`, `async fn` natif ailleurs.
- **Tester contre un vrai PostgreSQL en conteneur**, pas un faux dépôt en mémoire : un mock
  invente une sémantique que la base n'a pas.
- Valider à la conception plutôt qu'à l'exécution : un schéma incompatible se refuse avant de
  lancer le job, pas au milieu du traitement.

### Qualité et CI

- `cargo fmt --check`, `cargo clippy -- -D warnings`.
- **`cargo-deny`** (licences, doublons de versions, avis) et **`cargo-audit`**. En place dès
  l'étape 1 : gratuit maintenant, insoluble à l'étape 20.
- `Cargo.lock` **committé** — c'est une application, pas une bibliothèque.
- Côté front : lockfile committé, **`npm ci`** en CI, jamais `npm install`.
- Types générés committés, et **CI en échec si régénérer produit un diff**.
- Inventaire des licences archivé avec chaque release.

### Documentation

- Une décision d'architecture se consigne dans `doc/architecture.md` §8 avec une position,
  un motif, une date et l'étape où la trancher.
- **Une décision renversée est marquée comme telle**, pas réécrite en silence. On doit
  pouvoir lire ce qu'on croyait et pourquoi on avait tort.

---

## Ne pas re-proposer

Positions instruites puis abandonnées. Les remettre sur la table demande un fait nouveau.

| Décision | Position abandonnée | Pourquoi |
|---|---|---|
| **D4** | pas de données entre les nodes | le flux de données est le cœur du produit |
| **D10** | datasets matérialisés en v1 | plafonne à ~1,4 M de lignes à 350 colonnes |
| **D17** | opérateurs de nodes écrits à la main | c'est DataFusion en moins bien, sur des années |
| **D9** | Rhai évalué par ligne | 315 milliards d'appels d'interpréteur ; `Expr` est vectorisé |
| **D12** | `tiberius` pour SQL Server | 2 ans sans release ; `arrow-odbc` est meilleur techniquement |
| **D19** | UI en Rust/WASM (`egui`) | pas d'équivalent de CodeMirror, et le canvas n'est que 30 % de l'UI |

**Hors v1, par choix assumé** — ne pas élargir le périmètre sans décision explicite :
boucle `Tant que` (c'est un cycle, elle casse le tri topologique — la sortie propre est un
node conteneur) · `Map` paliers 2 à 4 · bouton IA sur SQL Input (validation DSI : le schéma,
jamais les lignes) · `Script` Python · Excel · SFTP · pagination HTTP · nodes `Union` /
`Sort` / `Aggregate` · déclencheur webhook.

---

## Où trouver le détail

| Document | Contenu |
|---|---|
| `doc/roadmap.md` | 24 étapes (0 à 23), jalon v0.5 après l'étape 13, critères d'acceptation |
| `doc/architecture.md` | modèle de données, schéma SQL, sémantique d'exécution, les 19 décisions |
| `doc/nodes.md` | spécification node par node : ports, config, erreurs |
| `doc/traits.md` | découpage en crates, traits, et ce qu'il ne faut pas abstraire |
| `doc/dependances.md` | état de maintenance, coûts, alternatives (daté) |
| `doc/react-depuis-angular.md` | transition Angular → React |

En cas de contradiction entre ce fichier et `doc/`, **`doc/` fait autorité** — il est plus
détaillé et plus à jour. Signaler la divergence plutôt que de choisir en silence.
