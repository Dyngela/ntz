# web — front React

**Volontairement vide pour l'instant.** Le front arrive à l'**étape 11** de
[la roadmap](../doc/roadmap.md) ; initialiser Vite maintenant laisserait un
`node_modules` et un lockfile dormir plusieurs mois, donc se périmer.

L'emplacement, lui, est fixé : tout le front vit ici, et rien ailleurs.

## Ce qui sera mis en place ici

Pile décidée (**D19**, [architecture.md §9.1](../doc/architecture.md)) :

| | |
|---|---|
| Build | Vite + TypeScript |
| UI | React 19 |
| Canvas | `@xyflow/react` (React Flow) — MIT, cœur gratuit |
| État client | Zustand (+ Immer pour le graphe) |
| État serveur | TanStack Query |
| Éditeur de code | CodeMirror 6 |
| Tests | Vitest + React Testing Library |

## Les deux règles à ne pas perdre de vue

**Aucun type d'API écrit à la main.** Ils sont générés depuis l'OpenAPI produit par
Rust, committés dans `src/types/generated/`, et la CI échoue si les régénérer
produit un diff. Renommer un champ dans une struct Rust doit casser `tsc`.

**Les configs de nodes ne sont pas typées statiquement.** Elles restent en
`unknown`, pilotées par le JSON Schema servi par `GET /api/node-kinds`. Les typer
recoupleraient le front au catalogue de nodes, alors qu'ajouter un node doit coûter
zéro ligne de React.

Le reste — la transition depuis Angular, les pièges de `useEffect`, les conventions
de dossiers — est dans [doc/react-depuis-angular.md](../doc/react-depuis-angular.md).
