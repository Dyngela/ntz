# React quand on vient d'Angular

Support de l'[étape 11](roadmap.md#étape-11--react--fondations-depuis-angular).

Le tooling, TypeScript, les composants, le typage des entrées/sorties : déjà acquis.
Rien à réapprendre là. Le travail réel est de **désapprendre quatre réflexes**, parce
qu'ils ne produisent pas d'erreur de compilation en React — ils produisent des bugs
silencieux.

React est plus petit qu'Angular. Il n'a pas de DI, pas de routeur, pas de client HTTP,
pas de gestion de formulaires, pas de couche réactive. Ces briques existent, mais on
les choisit. Le contrecoup : la doc officielle ne dit pas comment structurer une
application, il faut assumer ces décisions. Elles sont prises plus bas.

---

## 1. Les quatre désapprentissages

### 1.1 Muter un objet ne redessine rien

En Angular, `this.items.push(x)` finit par s'afficher : le dirty-checking compare des
valeurs à chaque cycle, la mutation est vue.

En React, l'état est comparé par **référence**. Muter en place ne change pas la
référence, donc React conclut que rien n'a bougé.

```tsx
// ✗ Ne redessine pas
const [nodes, setNodes] = useState<Node[]>([]);
nodes.push(newNode);
setNodes(nodes);            // même référence → aucun rendu

// ✓
setNodes(prev => [...prev, newNode]);
setNodes(prev => prev.map(n => n.id === id ? { ...n, x: 42 } : n));
```

C'est le bug n°1 des développeurs Angular en React, et il est particulièrement vicieux
sur un canvas de nœuds où l'on manipule des tableaux d'objets imbriqués en permanence.
Pour l'éditeur (étape 15), utiliser [Immer](https://immerjs.github.io/immer/) via le
middleware Zustand : on écrit du code mutatif, Immer produit l'objet immuable.

**Passer `setState` une fonction, pas une valeur**, dès que la nouvelle valeur dépend
de l'ancienne. `setCount(count + 1)` deux fois de suite n'incrémente que de 1 : les
mises à jour sont groupées et `count` est figé pour le rendu en cours.

### 1.2 Le composant entier se ré-exécute à chaque rendu

Un composant Angular est une classe : le constructeur tourne une fois, les méthodes
sont appelées à la demande. Un composant React est une **fonction rappelée
intégralement à chaque rendu**. Tout ce qui est déclaré dans le corps est recréé.

```tsx
function Editor() {
  const nodeTypes = { command: CommandNode };  // ✗ nouvel objet à chaque rendu
  return <ReactFlow nodeTypes={nodeTypes} />;  //   → React Flow remonte tous les nœuds
}

// ✓ déclaré hors du composant : référence stable
const nodeTypes = { command: CommandNode };
function Editor() {
  return <ReactFlow nodeTypes={nodeTypes} />;
}
```

Ce n'est pas un cas d'école : c'est le piège documenté n°1 de React Flow, et il coûte
directement la fluidité du canvas.

Corollaire : `useMemo` et `useCallback` ne sont pas des optimisations cosmétiques, ils
servent à **stabiliser des références** dont d'autres choses dépendent (props d'un
composant mémoïsé, tableau de dépendances d'un `useEffect`). Le
[React Compiler](https://react.dev/learn/react-compiler) automatise une grande partie
de ce travail ; l'activer sur ce projet est raisonnable, mais il faut comprendre ce
qu'il fait avant de le laisser le faire.

### 1.3 `useEffect` n'est pas `ngOnInit`

Le réflexe le plus coûteux. `useEffect` n'est pas un hook de cycle de vie, c'est un
outil pour **synchroniser React avec un système extérieur** : abonnement WebSocket,
timer, écouteur DOM, instance de bibliothèque tierce.

Il ne sert **pas** à :

| Réflexe Angular | Ce qu'il faut faire en React |
|---|---|
| `ngOnInit` → `this.http.get()` | TanStack Query (`useQuery`) |
| Recalculer un champ dérivé quand une entrée change | Le calculer directement pendant le rendu, ou `useMemo` |
| `ngOnChanges` → resynchroniser un état local sur une prop | Ne pas dupliquer la prop dans un état ; si vraiment nécessaire, la `key` du composant |
| Réagir à un clic | Le faire dans le gestionnaire d'événement |

Test simple : **si l'effet est déclenché par une interaction utilisateur, il ne doit
pas être un effet.** À lire une fois, en entier :
[You Might Not Need an Effect](https://react.dev/learn/you-might-not-need-an-effect).

Deux détails qui surprennent : la fonction retournée est le nettoyage
(`ngOnDestroy`), et en développement React monte / démonte / remonte chaque composant
une fois exprès, pour révéler les effets sans nettoyage. Un double appel en dev est
donc normal — et si ça casse quelque chose, c'est l'effet qui est en cause.

### 1.4 Pas d'injection de dépendances

Pas de `inject()`, pas de providers, pas de tokens. Un service Angular devient, selon
son rôle :

- **fonctions sans état** → un module TypeScript, importé directement ;
- **état partagé** → un store Zustand, importé directement ;
- **état serveur** → un hook TanStack Query ;
- **dépendance qui doit varier par sous-arbre** (thème, utilisateur courant, client
  API en test) → `useContext`.

`useContext` est le seul vrai équivalent de la DI, et il coûte des re-rendus : tout
consommateur se re-rend quand la valeur change. Il est fait pour ce qui change rarement.
Ne pas y mettre l'état du graphe.

Conséquence agréable : les tests n'ont pas de `TestBed`. On importe la fonction, on
l'appelle. Un composant se teste avec React Testing Library sans module de test à
configurer.

---

## 2. Table de traduction

### Template → JSX

| Angular | React |
|---|---|
| `*ngIf="x"` / `@if (x)` | `{x && <div/>}` ou `{x ? <A/> : <B/>}` |
| `*ngFor="let i of xs"` / `@for` | `{xs.map(i => <Row key={i.id} …/>)}` |
| `track i.id` | `key={i.id}` — **obligatoire**, et jamais l'index si la liste bouge |
| `[class.active]="x"` | `className={clsx({ active: x })}` |
| `[style.width.px]="w"` | `style={{ width: `${w}px` }}` |
| `{{ value \| currency }}` | `{formatCurrency(value)}` — pas de pipes, des fonctions |
| `<ng-content>` | `props.children` |
| `<ng-container>` | `<>…</>` (fragment) |
| `@Input() x` | une prop |
| `@Output() done = new EventEmitter()` | une prop `onDone: () => void` |
| `[(ngModel)]="x"` | `value={x} onChange={e => setX(e.target.value)}` |
| `@ViewChild('el')` | `const ref = useRef<HTMLDivElement>(null)` puis `ref={ref}` |
| Directive structurelle | Un composant, ou une prop de type fonction |
| Directive d'attribut | Un hook, ou des props étalées (`{...getProps()}`) |

`class` s'écrit `className`, `for` s'écrit `htmlFor`, les événements sont
`onClick`/`onChange` en camelCase et reçoivent un événement synthétique React.

### Réactivité

Angular a convergé vers les signaux, ce qui rend la correspondance assez directe —
mais avec une différence de fond : un signal notifie de façon ciblée, `useState`
déclenche le re-rendu **du composant entier et de son sous-arbre**.

| Angular | React |
|---|---|
| `signal(0)` | `useState(0)` |
| `computed(() => a() * 2)` | `useMemo(() => a * 2, [a])`, ou juste `a * 2` |
| `effect(() => …)` | `useEffect(() => …, [deps])` — mais lire §1.3 avant |
| `OnPush` | Le défaut. `React.memo` pour sauter un sous-arbre inchangé |
| `ChangeDetectorRef.markForCheck()` | N'existe pas. Un `setState` est le seul signal |
| `linkedSignal` / état dérivé d'une entrée | `key` sur le composant, pour le réinitialiser |

### Écosystème

| Angular | Choix pour ntz | Pourquoi |
|---|---|---|
| `HttpClient` + RxJS | **TanStack Query** + `fetch` | Cache, revalidation, états de chargement/erreur, invalidation. Remplace 80 % de ce pour quoi on utilisait RxJS. |
| Service avec `BehaviorSubject` / NgRx | **Zustand** | Store hors React, minimal, pas de boilerplate. C'est aussi le pattern recommandé par React Flow. |
| Reactive Forms | **react-hook-form** + Zod | Non contrôlé par défaut, donc pas de re-rendu à chaque frappe. Pour l'éditeur, les formulaires sont générés depuis le JSON Schema du nœud. |
| `RouterModule`, guards, resolvers | **React Router** (v7) | Les *loaders* remplacent les resolvers ; un guard devient un composant enveloppant. |
| Angular Material | **shadcn/ui** + Tailwind | Composants copiés dans le projet, pas une dépendance opaque. |
| `TestBed` + Karma/Jasmine | **Vitest** + React Testing Library | Rien à configurer par test. |
| Angular CLI | **Vite** | Déjà connu. |
| RxJS pour la coordination | À éviter au début | Tentation forte quand on vient d'Angular. Les promesses et TanStack Query couvrent le besoin ; garder RxJS pour un flux réellement continu (le SSE de l'étape 14 est un candidat légitime, et le seul). |

---

## 3. Nouveautés React 19 à connaître

Utiles ici, et absentes des tutoriels plus anciens :

- **`ref` est une prop normale.** `forwardRef` n'est plus nécessaire (et est déprécié).
- **`use(promise)`** déballe une promesse pendant le rendu, avec Suspense.
- **`useOptimistic`** pour l'UI optimiste — pratique sur le déplacement de nœuds : on
  affiche immédiatement, on annule si le serveur refuse.
- **`useActionState`** pour les soumissions de formulaire (état en attente + erreur
  sans état manuel).
- **React Compiler** insère la mémoïsation automatiquement. Comprendre `useMemo` /
  `useCallback` d'abord, l'activer ensuite.

---

## 4. Parcours

Compter une quinzaine d'heures pour §1–§4, l'essentiel étant la désintoxication de
`useEffect`.

1. **[react.dev/learn](https://react.dev/learn)**, en entier. C'est court, et écrit
   pour la version actuelle de React. Ne pas apprendre React sur des tutoriels
   antérieurs à 2023 : les composants de classe, `componentDidMount` et les HOC ne
   sont plus le sujet.
2. **[Thinking in React](https://react.dev/learn/thinking-in-react)** et
   **[Sharing State Between Components](https://react.dev/learn/sharing-state-between-components)** — le
   remontage de l'état, ce que la DI dispensait de faire.
3. **[You Might Not Need an Effect](https://react.dev/learn/you-might-not-need-an-effect)** —
   le texte le plus rentable de la liste.
4. **[Escape Hatches](https://react.dev/learn/escape-hatches)** — refs, effets,
   `useSyncExternalStore`. C'est ce sur quoi repose l'intégration de React Flow.
5. **[TanStack Query](https://tanstack.com/query/latest/docs/framework/react/overview)**,
   les concepts : clés de requête, `staleTime`, invalidation, mutations.
6. **[Zustand](https://zustand.docs.pmnd.rs/)** — une demi-heure de lecture suffit.
7. **[React Flow](https://reactflow.dev/learn)** — le guide *Learn*, puis les exemples
   *Custom Nodes*, *Validation*, *Prevent Cycles* et *State Management with Zustand*.
   Ces quatre exemples sont, à peu de choses près, l'étape 15.

**Vérification de l'acquis** avant de démarrer l'étape 15 : savoir expliquer pourquoi
`setNodes([...nodes])` redessine mais `nodes.push(n); setNodes(nodes)` non, et pourquoi
déclarer `nodeTypes` dans le corps du composant dégrade le canvas.

---

## 5. Conventions pour le front de ntz

À figer avant d'écrire la première ligne, sinon la dette arrive vite.

```
web/
  src/
    api/          # client fetch typé + hooks TanStack Query, un fichier par ressource
    components/   # présentation réutilisable, sans état serveur
    features/
      workflows/  # liste, détail, plannings
      editor/     # canvas, store Zustand, nœuds custom, formulaires JSON Schema
      runs/       # historique, détail d'un run, logs SSE
    lib/          # utilitaires purs
    types/        # types générés depuis l'API
```

- **Un dossier par fonctionnalité**, pas par type de fichier. Le découpage
  `components/services/models` d'Angular ne passe pas l'échelle ici.
- **Types générés, pas écrits à la main** (**D19**). Le backend expose un OpenAPI
  (`utoipa` côté axum), le front génère ses types dans `src/types/generated/`, et la CI
  échoue si régénérer produit un diff. Deux définitions parallèles d'un même contrat
  finissent toujours par diverger. Renommer un champ en Rust doit casser `tsc`.
- **Sauf les configs de nodes**, qui restent en `unknown` et pilotées par le JSON Schema
  servi par l'API — les typer statiquement recoupleraient le front au catalogue de nodes
  (architecture.md §9.1).
- **Aucun `any`.** `unknown` + un parseur Zod à la frontière.
- **`useEffect` avec un fetch dedans = revue de code refusée.** C'est la règle qui
  concentre le plus de valeur.
- **Composants de présentation sans accès réseau.** Ils reçoivent des données en props ;
  les hooks de requête vivent dans les composants de `features/`.
