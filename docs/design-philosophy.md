# Design Philosophy: Voxel Factory Sandbox

Oct 3, 2026 · @Mariano Sorgente

## Vision and north star

We are building an open-source voxel sandbox about technology, exploration and mastery: Minecraft's world, Factorio's factories, RuneScape-style interlocking skills, and procedurally generated places, mechanics and cultures that make sense. It is written in Rust on Bevy, runs fast on macOS and every other platform, supports multiplayer from one client, and is fully moddable by humans and AI.

It is a game first and a creation platform second. The base game must be fun, polished and visually coherent on its own. Underneath it, the exact same systems are open to human mods, AI authors and procedural generation.

The long-term goal is Minecraft-like systemic depth, Roblox-like low-friction sharing, a modern open mod architecture, AI-native creation, and causal procedural worlds. The engine provides a stable grammar; humans, mods and AI provide the vocabulary.

The game should make players feel three things in order:

1. "I wonder what's over there."
2. "I've never seen this before."
3. "Of course this exists here."

The third reaction is the target. Not infinite randomness, but infinite, understandable consequence, and new things to *do*, not just new things to look at.

## Motivations

Three well-known games each show a limit this game is designed around.

| Game | What it gets right | Where it hits a wall | What we do instead |
| --- | --- | --- | --- |
| Minecraft | A universal voxel grammar; building and destruction anyone understands; a huge mod scene | One main simulation thread, heavy allocation and GC pauses, a legacy OpenGL renderer (translated to Metal on macOS), and mods that compete to patch the same methods | Rust and wgpu (native Metal on Mac), a parallel region-based simulation, event scheduling instead of ticking everything, and mods that add vocabulary instead of patching |
| No Man's Sky | Endless variety of places and creatures | Procgen produces nouns (planets, colours, species) while the verbs stay the same; after a few hours every planet offers the same decisions | Generate decisions, not just nouns: new machines, trade styles, games, behaviours and tools, proven by bots to have depth and to be genuinely new |
| Factorio | Deep, interlocking production from a few quantities; excellent performance through sleeping entities and merged belt lines | A fixed, hand-made world and content set | Factorio-grade simulation techniques inside a causal, generated, moddable world |

Two further motivations shape the design:

- **AI changes who can make mods.** If content is typed data in a small, checkable language, an LLM can author it reliably, and players can describe what they want and get a working, validated package.
- **Variety must be structural.** Players quickly recognise the grammar behind repeated content. Two towns built from the same building grammar feel different only if one is a poor frontier mining village and the other is a wealthy trade capital with a different history, economy and way of trading.

## Core design principles

These principles decide most arguments before they start.

1. **Constraints create coherence.** Unlimited freedom produces incoherence. A voxel world gives a universal spatial grammar: simple building and destruction, natural chunking, deterministic generation, predictable collision and a strong visual identity. Not everything is a cube: creatures, machines and plants use standard meshes, rigs and budgets. Voxel world, constrained meshes, strong visual identity.
2. **Keep the engine small; make it a grammar.** The engine holds only what every world needs. Gameplay is expressed in it, not built into it. A mod written years from now should not care which renderer or ECS sits underneath.
3. **Seven primitives, nothing more.** Everything in the game is a Definition, Trait, Field, Action, Process, Agent or Generator. If an idea doesn't fit, rethink the idea before adding a primitive.
4. **First-party content is built like mods.** The furnace, conveyor, wolf and tutorial quests use the same APIs as any creator. If the base game can't be built cleanly with the frameworks, the frameworks are incomplete.
5. **Extend by adding, never by patching.** Mods add actions, traits, roles and modifiers. An NPC gains new behaviour because a train mod adds a `board_train` action it can discover, not because someone rewrote the villager. Load order never defines reality.
6. **Procgen produces causality, not decoration.** Every generated place must answer "why is this here?" A city is wealthy because it has resources, trade access and a history of using them.
7. **Reuse before invention.** Generators walk the ladder reuse → parameterize → compose → subtype → invent. Iron with purity 0.63 beats `MountainIron_8712`. The number of types should grow far more slowly than the amount of content.
8. **Generate decisions, not just nouns.** A new mechanic must create a real choice with verifiable depth. New items with bigger numbers are not gameplay.
9. **Innovation is a search process.** LLMs propose and mutate; bots, evaluators and an archive decide what survives. "New" is measured on how people play, not on names or art.
10. **Structured simulation is truth; AI is expression and authorship.** The running world never depends on an LLM to exist or to be reproduced.
11. **Validate the composed world.** A package can be valid alone and broken in combination. Everything is simulated before anyone plays it.
12. **Use the cheapest update that works.** A 30-second smelt is one scheduled event, not 600 ticks. Idle things sleep.
13. **Determinism is a design goal.** Replays, debugging, validation and reproducible bugs depend on it.
14. **The engine library is an implementation detail.** No Bevy types in the public API.
15. **The base game must be excellent.** User content expands a great game; it can't excuse a missing one.
16. **Everything connects.** Every system reads from others and feeds others: biomes decide plants, plants feed farming, cooking and alchemy, food and potions answer the weather and the terrain. A definition nothing uses is a dead end. See "Everything connects" below.
17. **Slightly fantastic, never arbitrary.** The world looks like a real place first. Rare fantastical plants, trees, materials and places appear everywhere, more in fantasy biomes, and always for a reason the world can explain.
18. **Every presentation layer is replaceable.** Texture pack, shader pack, shader pack loader, sky and clouds, particles and ambience each sit behind a small interface; the first-party versions are just the default packs.

## The model: seven primitives

Agents choose Actions. Actions change Traits. Processes evolve Traits and Fields over time. Generators create all of it. Packages ship all of it.

| Primitive | What it is | Examples |
| --- | --- | --- |
| Definition | A named template for a kind of thing (block, item, entity, fluid). Pure data. | `base:furnace`, `base:iron_ore`, `base:wolf`, `bees:hive` |
| Trait | Typed data attached to a thing; its "attributes". Declares storage, replication and persistence. | `core:heat`, `core:inventory`, `core:skills`, `base:crop`, `bees:colony` |
| Field | A trait of space, stored coarsely per cell. The main way loosely related systems influence each other. | temperature, humidity, pollution, danger, wealth, pollen |
| Action | The only way the world changes in discrete steps: requirements, inputs, outputs, duration, effects, skill. Recipes are actions. | `core:mine`, `base:smelt_iron`, `core:transfer`, `town:buy`, `bees:harvest_honey` |
| Process | A continuous rule over traits and fields at a scheduled rate. Never decides; emits events at thresholds. | crop growth, heat, power networks, populations, spoilage |
| Agent | Anything that chooses actions. Only the controller differs. | player (input), furnace (loop), wolf (utility), villager (planner), village (strategic) |
| Generator | A deterministic function from seed, scope and inputs to things, traits and fields. | geology, climate, ore deposits, settlements, wild hives |

Two organising ideas sit on top, without being primitives:

- **Role:** a named query, such as `transport:Provider` or `town:FoodSource`. Systems ask "what can do X?" instead of "is mod Y installed?". Roles are inferred from what things actually have, so nothing can falsely claim one. This replaces the original "capabilities" and stops mods from needing pairwise integrations.
- **Package:** the unit of distribution. A framework is simply a package whose main job is to define traits, actions, roles and processes for one domain.

What the original concepts became:

| Original term | Now |
| --- | --- |
| Nouns | Definitions |
| Attributes, components | Traits |
| Verbs, recipes, machine processes, NPC actions | Actions |
| Capabilities | Roles |
| Fields and signals | Fields (signals are events emitted by actions and processes) |
| Systems, feedback loops | Processes |
| Frameworks, mods, generated packages | Packages |

"Hardcode verbs more than nouns" survives in sharper form. The engine knows a small, fixed set of **effects** that every action compiles down to: spawn, despawn, set voxel, set or add a trait, apply a modifier, transfer items, give or take items, emit into a field, raise an event, schedule. Consume, produce and transform are just transfers and trait changes. Because only these effects touch the world, the engine can make every action atomic, validated, deterministic and networked, whoever wrote it.

Most logic is written in a small, pure, typed **expression language** (`$target.mineable.hardness / $actor.held.tool.speed`). When that isn't enough, any expression slot can call WASM.

## How the primitives relate

Four rules carry most of the architecture:

1. **Only agents decide.** If something makes a choice, it's an agent.
2. **Only actions make discrete changes.** Every block mined, item crafted, trade made or NPC hired goes through one lifecycle: propose, validate, reserve inputs, schedule one completion, then commit atomically or cancel. One place for validation, networking, logging and prediction.
3. **Processes are continuous and never decide.** Crops grow, heat spreads, power flows. At a threshold a process emits an event; an agent or action does the rest.
4. **Generators only run at creation time.** Afterwards, the world changes only through actions and processes.

|  | Reads traits | Writes traits | Reads fields | Writes fields | Creates or destroys things | Decides | Runs |
| --- | --- | --- | --- | --- | --- | --- | --- |
| Agent | yes (senses) | no | yes | no | no | yes | at its tier |
| Action | yes (requirements) | yes | yes | point emissions | yes | no | when chosen |
| Process | yes | yes | yes | yes | no | no | at its tier |
| Generator | earlier stages | initial values | yes | baselines | yes | no | at generation |

Three mechanisms make independently written packages compose:

- **Agents discover their options.** An agent never holds a hardcoded list of things it can do. It asks an incremental action index which actions its traits, held items and surroundings allow. When a mod adds `board_train` tagged `travel`, every NPC planner that needs to travel starts using trains.
- **Fields decouple systems.** Pollution reduces crop growth through a modifier; neither system names the other. A mod can add a new field, and existing systems react if they read it. This is how the world becomes deeply interconnected without an O(n²) web of dependencies.
- **Modifier stages replace load order.** Every modifiable value resolves through fixed stages: base, add, multiply, clamp, override. Within a stage only order-independent operations are allowed. Crop growth is base rate × climate × soil × pollution × pollination, each from a different package, and the result never depends on which loaded first. Override is rare, declared, and makes a package incompatible by default with others touching the same value.

A worked example: a player hand-smelting iron, a furnace smelting it automatically, and an NPC smith smelting it to earn money all perform the same `base:smelt_iron` action. Hand crafting, automation and NPC economies are one system, not three.

## Everything connects

The world is one system seen from many angles. Each new system must read from existing ones and feed into them, so that a player who learns one causal link finds it paying off somewhere else.

The backbone the base game ships:

- **Climate and geology decide biomes.** Temperature, moisture, terrain, rock and the world's aether (see "Slightly fantastic" below) choose the biome.
- **Biomes decide what is there.** Which plants, trees and fungi grow, which stone, clay and ore lie beneath, which creatures live there, and how often something strange turns up.
- **Plants are ingredients, not decoration.** Every plant carries properties (nutrition, flavour axes, alchemical properties, dye, fibre, fuel) instead of hardcoded recipes, so foraging, cooking, herbalism and alchemy all read the same data.
- **Farming moves plants out of their home, at a cost.** Growth is base rate × climate fit × soil × water × season × pollination. A frost-loving herb needs a cold field, a cellar, or a heated plot.
- **Food and potions answer the world.** They give buffs and resistances; weather and terrain create the needs. Warmth matters on a mountain in a blizzard, heat resistance in a desert at noon, water breathing in a flooded cave.
- **Weather is a field.** It changes crop growth, comfort, fire, visibility, which creatures are about, and what you hear and see.
- **Skills tie it together.** Foraging teaches which land grows what; farming extends it; cooking and alchemy turn it into an edge for mining, exploring and building.

**How, without a web of special cases.** Nothing links to anything by name. Biomes, plants, crops, foods, potions and buffs are Definitions with tags and properties. Climate, weather, soil, light and aether are Fields any system can read. Effects are modifiers at stages, so "+2 warmth in cold weather" from a stew and "−30% growth outside its climate" from farming compose without knowing each other. Roles find partners: anything with an alchemical property is usable by every brewing action, including ones a mod adds later.

**Dead ends fail validation.** A biome must contribute at least one ingredient, material or creature that some action uses. A plant must have at least one use beyond looks. The structural-gap check that guides generation (items with no use, fields with one reader) applies to handwritten content too.

## Slightly fantastic

The world should look like a real place first; fantasy is seasoning, and it follows causes like everything else.

- **Everywhere, rarely.** Ordinary biomes hold rare fantastical things: a ring of glowing mushrooms, a tree with silver bark, a vein of an unknown ore, a flower that only opens at night. Rare enough that each one is a discovery.
- **Fantasy biomes, rarer still.** Regions where the strange is the norm (crystal groves, glowing fungal forests, ember woods) have far more fantastical flora, fauna and materials, but are still anchored to a real climate: a crystal grove is cold, an ember wood hot and dry.
- **Aether is a field.** How fantastical a place is follows a world field, aether, which varies by region like rainfall and has a world dial for its overall level. Where aether is high the strange is common and alchemy is stronger, and players can learn to read it.
- **Everything strange does something.** Each fantastical plant, tree or material is an ingredient, a material, a light source or a hazard.

## Layers: what lives where

Everything above the engine is a package built the same way; only the engine is native code, and it sits beneath a stable API.

```
Platform          accounts, discovery, package registry (separate project)
Worlds            generated and played
Mods              anyone, including AI
Base game         core team, mostly data
Core frameworks   schemas in packages, hot loops as native kernels
─────────────── stable semantic API ───────────────
Engine            native Rust
Libraries         Bevy, wgpu, …
```

Read it bottom up: libraries can be swapped without touching anything above the dashed line, and frameworks, the base game, mods and generated worlds all share one package format and one set of rules.

| Layer | Who writes it | Can mods change it? |
| --- | --- | --- |
| Engine | core team, native Rust | no; replaceable beneath the API |
| Core frameworks | core team: schemas in packages, hot loops as native kernels | extend, don't replace |
| Base game | core team, mostly data | yes, through modifiers |
| Mods | anyone, including AI | not applicable |
| Worlds | generators and players | by playing |
| Platform | separate project | not applicable |

This settles the engine-versus-mod question for performance: **schemas live in packages, hot loops live in the engine.** A belt tier or a new power producer is data; the solver that moves items and power is native.

## Agents, skills and factories

Players, machines and NPCs are all agents performing the same actions; that single idea unifies the game's three pillars.

| Agent | Controller | How it chooses | Typical tier |
| --- | --- | --- | --- |
| Player | input | options come from input, validated like any other agent | high |
| Furnace, inserter | machine loop | first valid action from a list, re-decides only when inputs change | event-driven |
| Wolf | utility | scores options with considerations (hunger, light, threat) | medium near players |
| Villager, smith | planner | chains actions to satisfy goals (eat, earn, sleep) | medium |
| Village, faction | strategic | the same planner at very low frequency, over group actions | very low |

**Factories** come from machine agents running actions continuously, plus native network kernels for belts, fluids and power. Machines sleep when idle, and long actions are one scheduled completion.

**Skills** (RuneScape-style) are a core framework built entirely from primitives: a `core:skills` trait holds XP; actions declare a skill requirement, an XP grant, and level-based modifiers to speed, yield or quality. Cross-skill interaction comes for free because actions chain through items: mining feeds smithing, smithing feeds engineering, and engineering builds the machines that automate mining.

The default rule for automation: machines grant no XP, but building and configuring better machines is itself gated by skill. Mastery is what lets you automate, and automation frees you to train the next skill.

**NPCs have structured state** (profession, skills, wealth, home, workplace, relationships, needs, goals, knowledge). Behaviour emerges from that state and the shared actions. LLMs may express it in dialogue and summaries, but never invent facts about the character.

**Simulation level of detail.** Near players, a wolf pack is individual agents. Far away it's a number in a regional population process. When a player approaches, the number becomes packs again. The same definitions at two levels of detail are what let a causal ecology run across an infinite world.

## World generation

Proven algorithms build the physical world, LLMs give it meaning, and seeded dials drive both.

**Causal, layered generation.** No single algorithm generates the world. Generators run in stages, each reading only earlier stages: geology → climate → water → ecology → resources → terrain → caves → structures → settlements → decoration. Randomness fills structure; it never replaces it.

**World dials.** Each world samples about 30 physically meaningful values from its seed: axial tilt, ocean fraction, tectonic activity, volcanism, erosion age, humidity, wind model, an ore mix, biodiversity, number of cultures, conflict level, trade connectivity. An archetype (archipelago, ice age, volcanic, supercontinent and others) is sampled first and shifts the ranges; correlations prevent impossible combinations. An iron-poor volcanic copper world and a cold, crowded archipelago with no coal play differently before any LLM is involved.

**Proven techniques for physical structure:**

| Layer | Technique |
| --- | --- |
| Continents | Plate simulation (plates, motion, boundary types) |
| Elevation detail | Domain-warped fBm and ridged noise, masked by plates |
| Erosion | Hydraulic and thermal erosion on a coarse heightmap |
| Climate | Latitude and altitude lapse rate, wind bands, moisture advection, rain shadows |
| Biomes | Data definitions that each claim a box in climate space (temperature, moisture, continentalness, erosion, weirdness, aether, rock province), with blended borders and rare sub-biomes carved from a parent |
| Rivers | Springs traced downhill over the height field, joining into rivers that widen with each tributary; lakes fill depressions to their spill point; real flowing water |
| Caves | Noise caverns, winding and narrow tunnels, rare ravines, local water tables, and cave biomes chosen by depth, climate, rock and aether |
| Decoration | Per-biome rules in a fixed order, densities modulated by noise so forests come in stands with clearings, plus fallen logs, stumps, bushes, rocks and flower drifts |
| Ores | Rock type from plate boundaries and depth; ores by rock type |
| Scatter, plants | Poisson disk sampling; L-systems |
| Buildings | Shape grammars and wave function collapse with cultural tile sets |
| Settlements, roads | Agent-based history at coarse time; A\* over cost fields |
| Names | Generated phonologies per culture |

**Interestingness checks** reject dull or broken worlds (too uniform, no metals or water near spawn, blocked progression) and resample deterministically.

**World generation is compressed history.** Instead of placing a wealthy city, the settlement generator finds good sites (river crossings, fertile soil, deposits, harbours) and fast-forwards strategic agents performing real actions for a few hundred simulated years. The city's layout, wealth, architecture and culture then have causes. A mod that adds settlement actions (a port, a temple) changes how towns develop without editing the generator.

**Determinism.** Every random stream is seeded from the world seed, generator id and version, stream name and coordinates. Generation order and thread count never change the result. Chunks record which generator versions made them, so updated generators only affect newly generated land.

## The generation ladder and the role of LLMs

LLMs author packages; the engine runs packages; nothing in the running world depends on an LLM.

| Rung | Content | When | Who |
| --- | --- | --- | --- |
| 1. Instances | terrain, deposits, NPCs, settlements | runtime, constantly | engine generators |
| 2. Parameters | ore purity, wolf aggression, building proportions | runtime | generators |
| 3. Compositions | building grammars, behaviour sets, quest chains, recipes from existing ingredients | world creation or runtime, validated | generators using LLM-authored templates |
| 4. New definitions | a material, species, machine, skill | world creation or offline | LLM authoring, frozen into a package |
| 5. New code | a process, controller, game or generator in WASM | offline | LLM or human, through the normal mod pipeline |

Rungs 1–3 are computed from the seed. Rungs 4–5 are frozen into hash-addressed packages before anyone loads the world, so clients need only the seed and package hashes, never an LLM.

**Generated concepts have scope** (entity, chunk, region, biome, civilisation, world, global) and spread causally. If one region invents windmills, another adopts them because trade reached it, not because it was generated later.

**Where AI is used, and where it isn't.** AI handles semantic work: world themes, cultures, composing existing vocabulary, authoring mods, generating mechanics, dialogue, explaining the world, and repairing content. Deterministic systems handle physics, exact world realisation, networking, chunk generation, routine simulation, persistence and NPC execution. Natural language acts like a high-level programming interface; the deterministic engine is the compiler and runtime underneath.

**The world is queryable.** An ontology API answers "what produces iron ore?", "what modifies crop growth?", "why is this city wealthy?", "which packages touched this chunk?" with structured data. AI tools reason over explicit state instead of guessing from source code.

**Diverse LLM output comes from the process, not from temperature.** Every LLM call receives seeded cards: a real-world grounding (Icelandic geothermal baking, lost-wax bronze casting), a creative constraint ("nothing here may depend on iron"), a persona, a banned-cliché list, and a list of what this world already contains to contrast against. Prompts give specific facts and ask the model to explain and elaborate; they never ask open questions like "make a culture". Generation also targets structural gaps the ontology can detect, such as items with no use, fields with one reader, or skills with dead ends.

**At runtime, LLMs only express.** NPC dialogue, books and descriptions are built from existing facts, cached, and have templated fallbacks. The game is fully playable with the LLM switched off.

Food shows how far composition goes. Ingredients carry categories, nutrition, shelf life and seven flavour axes; techniques (bake, ferment, smoke, dry) transform them and depend on local temperature, humidity and stations. A generator builds each culture's signature dishes from local ingredients and techniques, stats are computed with balance caps, and the LLM only names and describes them.

## Generating gameplay: the innovation engine

The unit of generation is a mechanic, a new decision with verified depth, and new mechanics come from an open-ended search, not from prompts.

**Gameplay = decisions × feedback × mastery.** Every mechanic, generated or handwritten, has a spec stating its decision in one sentence, the concepts it reuses and introduces, and three depth layers (surface, skilled, expert), each written as a testable bot policy. A mechanic that can't fill these in honestly is rejected.

**Mechanics include far more than machines:** machines with new operating rules, trade styles (barter on gift history, sealed salvage auctions, delivery contracts), games played with NPCs, creature and NPC behaviours that create opportunities, situations driven by world processes, and quality-of-life tools learned from cultures.

**Bots can play everything.** Generated WASM implements generic SDK interfaces (`Game`, `Operable`, `Market`) so the engine supplies UI, NPC opponents and automated playtesting.

**Depth is proven.** Bots measure the skill gap (strong vs weak search), whether decisions are genuinely contested, seat fairness, dominant moves, and robustness under fuzzing. Each depth layer's policy must beat the previous layer's. A fresh LLM must be able to learn the mechanic from its in-world explanation alone. Cameron Browne's Ludi system evaluated generated board games this way and produced *Yavalath*, a published game.

**Novelty is measured on behaviour.** Asking a model to innovate returns its most typical answer, and a pattern library only produces reskins. So:

- **Reskin detector:** if an archived mechanic's expert bot, lightly tuned, plays a candidate near-expert, the candidate is a reskin.
- **Behaviour signature:** what expert play is about (timing, geometry, information, trust), its timescale, spatial footprint and what it changes.
- **Interestingness judge:** an LLM, shown the archive and playtests, judges whether it is interestingly new (the OMNI approach).
- **Prior-art check** against known game mechanics.

**The open-ended loop.** Building on novelty search, POET, Evolution through Large Models, OMNI-EPIC, Voyager, FunSearch and AlphaEvolve, an offline loop grows a global quality-diversity archive of mechanic genomes (spec, data, WASM source, tests, lineage). The LLM acts as a mutation operator, applying design moves to parents' code: remove an assumption, invent a quantity, change what the player acts on, make it social, change the timescale, make the tool the resource, invert the beneficiary, crossover, exaggerate a side effect, simplify. Behaviourally new but imperfect mechanics are kept as stepping stones.

**Invent quantities, not just rules.** Depth comes from a few interacting quantities. The loop may create new fields with their own dynamics (sound that reflects off blocks, values that spread by imitation between NPCs) that later mechanics build on.

**Mechanic mining.** Curiosity-driven agents explore each world's simulation for unplanned high-leverage, nonlinear, cyclic or cascading dynamics. The LLM then adds a lever and feedback to make them deliberate and legible. Wavedashing, rocket jumps and redstone computers were found in systems, not designed.

An example lineage: a vent forge (timing) became a listening forge (a reskin, kept as a stepping stone), then a reflecting sound field, then tuned caves where players mine and place blocks to move standing-wave peaks onto mineral deposits. That passes the reskin test because expert play is geometric. In a world with bees, mining found that swarms feed the same field, making colonies part of the instrument.

Each world gets mechanics from the global archive, from mutations conditioned on its own facts, and from dynamics mined from its own simulation. Players' undocumented techniques, strange builds and community mods feed the archive too.

## Depth without overwhelm

The world may become extremely deep, so players learn it one causal link at a time and only the core is ever required.

**Technology is the core fantasy.** The base game ships one excellent loop: discover → extract → transport → process → manufacture → power → automate → expand → explore for new constraints. Maximum fun from minimum canonical content: a small number of deeply composable systems, not hundreds of special-case machines.

**Three tiers of mechanics:**

| Tier | What | Required | New concepts each | Frequency |
| --- | --- | --- | --- | --- |
| Core | The handwritten game: mining, smelting, belts, power, skills, basic trade | yes | about 25 in total, taught in order | everywhere |
| Regional | A twist attached to a culture, biome or creature | no, but offers better or alternative routes | at most 1 | a few per culture or biome |
| Discovery | Rare, deep, hidden mechanics | no | at most 2, building on known ones | a handful per world |

**The concept budget.** The game tracks which concepts each player knows. Once learned, a concept can be reused by later mechanics for free, so learning compounds. Regional mechanics should solve problems the world created: in an iron-poor world, local bronze casting is the answer to "where do I get good tools?", not extra homework.

**Quests teach the simulation.** Find copper (geology), smelt it (machines, heat, recipes), move it (logistics), power the crusher (energy), workers refuse to move (housing, food, safety), the city congests (transport). Eventually the player realises these were never separate systems.

**Teach through agents, not pop-ups.** Because NPCs perform the same actions as players, they demonstrate: you see Varn smiths watching a gauge and pouring, a smith explains in three lines, and a small job lets you try it.

**A discovery director paces novelty per player.** A server-side strategic agent reads known concepts, play style and time since the last new concept, then surfaces mechanics through rumours, demonstrations and quests. It slows down if rumours are ignored. Target: about one new concept per one to two hours for an engaged player.

**Surprise comes from connections.** A bridge pass links each world's mechanics so that one mechanic's expert insight pays off in another. Discovery-tier secrets are locked behind understanding, not keys. Quality-of-life tools (route cairns, pack animals, tide tables) are rewards for exploring cultures, because they reduce load instead of adding to it.

## Modding, composition and trust

Mods can add all seven primitives; what they can't add is the engine's fixed vocabulary underneath.

**One system for everyone.** The mod API and the procedural-generation API are the same semantic system. First-party developers, human modders, AI authoring tools and world generation all produce versioned packages with a namespace, version, dependencies, schemas, permissions, resource budgets, tests and provenance. There is no privileged AI path, and no generated system is mysterious internal state.

**Data first, code when needed.** Most content is RON data plus expressions; the beekeeping sample mod is about 85% data. WASM is the execution target for the rest: Rust today, a TypeScript-like language later. Every WASM function reads a snapshot and returns proposals (effects, writes, choices, generated content); the host validates and applies them. That one rule gives sandboxing, determinism, parallelism and replay. Processes receive batches so the WASM boundary is crossed once per batch.

**Changing other packages' content without patching:**

| Want to | How |
| --- | --- |
| Add a trait to existing definitions | `attach_to` a tag or query |
| Change a value | A modifier at a stage |
| Add a behaviour option | An action whose requirements existing agents meet |
| Add a tag to others' definitions | A tag extension |
| Add to a native network | Definitions with that kernel's node trait |
| Replace something | `override`: declared, rare, incompatible by default |

**Engine-only, never moddable:** the effect vocabulary, native kernels (belts, flow networks, heat, fluids, light), scheduling tiers and phases, modifier stages, replication modes, expression built-ins, the render backend, physics and the network protocol. What the renderer draws with (textures, shader packs, sky and clouds, particles) is data in packages; see "Visual identity and rendering". When many mods want the same missing capability, that's the signal to extend the engine; promising generated mechanics can file engine proposals for human review.

**WASM is the security boundary.** Packages declare least-privilege permissions (which definitions they may spawn or place, which values they may modify, which fields they may emit into) and budgets for CPU, memory and fields. No arbitrary files, network, native code or global-economy access. Determinism is enforced: no clocks, threads or host randomness, NaN canonicalisation, fuel metering, and replay tests before publishing.

**Generated code gets extra checks:** interface-only crates, size limits per tier, LLM-written property tests checked under fuzzing, 10,000 random plays that must terminate and replay identically, review by a second model, and a server-side kill switch that degrades a misbehaving mechanic's items to core equivalents.

**Expansion is balanced by compression.** Generated variants are periodically merged into parameterised definitions (ForestWolf, MountainWolf and ColdWolf become one wolf with parameters). Popular regional packages can be promoted to global ones.

## Simulation, performance and multiplayer

The simulation is built to be cheap, parallel, deterministic and server-authoritative from day one, which is where Minecraft is hardest to fix.

**Schedule by need, not one tick rate.**

| Tier | Examples |
| --- | --- |
| High | player movement, nearby physics, combat |
| Medium | active machines, nearby NPCs |
| Low | ecology, settlement updates, fields near players |
| Very low | distant regions, populations, economies |
| Event-driven | action completions, furnaces, logistics events |

Every process also declares what happens far from players: keep running (factories and power, so automation works while you explore), run slower, hand over to a cheaper abstract process (wolves become a population), or freeze and catch up analytically on load.

**Performance techniques:**

- Idle things sleep; a 30-second smelt is one scheduled event.
- Belts merge into transport lines; power, fluid and item networks are solved per network, not per block, by native kernels.
- Plain blocks are only a palette index in a 32³ chunk with variable-width packing; a block becomes an ECS entity only if it has changing per-instance state.
- Fields are coarse grids (often 16 blocks per cell) and save only differences from their generated baseline.
- Generation, meshing, lighting and pathfinding run on worker threads.
- Independent regions simulate in parallel; cross-region effects are explicit messages.
- Profile from the start with Tracy, plus Xcode Instruments on macOS, and test on low-end machines.

**Determinism is explicit:** fixed timestep, seeded RNG streams per system, no hash-order dependence, deterministic WASM. It enables replay, rollback, debugging, validation and reproducible generated-world bugs.

**Networking is server-authoritative.** Clients send intent (proposed actions); the server validates, simulates and commits; clients may predict actions marked predictable. Each trait declares its replication (interpolated, reliable, owner-only, server-only), so nothing syncs by accident. Chunks sync as seed baseline plus authoritative differences, because client and server can rebuild the baseline from the seed and package versions.

Voxel players spread across a huge world, so lockstep (Factorio's model) would force every client to simulate every loaded region. Server-authority with interest management is the default; keeping the simulation deterministic leaves lockstep open later.

**One client, any server.** On joining, the client compares the server's package hashes with its cache and downloads what's missing. Sandboxed WASM is what makes auto-downloading safe.

## Visual identity and rendering

Beautiful means coherent and intentional, not photorealistic, and generated content must look like it belongs.

**Rendering approach:**

- wgpu through Bevy: native Metal on macOS, Vulkan and DirectX elsewhere, so there is no deprecated OpenGL path.
- Binary greedy meshing on worker threads with vertices packed into 4–8 bytes.
- Pixel-art textures in a texture array with nearest sampling and repeat wrapping, so merged faces still tile.
- Frustum culling plus a per-chunk visibility graph so unseen caves don't render; GPU-driven or indirect drawing where supported.
- Lower-resolution distant chunks (Distant Horizons style) for long views.
- 4-bit sky and block light, flood-filled incrementally off the main thread.

**Keeping generated art consistent.** Diffusion-model images drift in style and are hard to keep pixel-perfect. Instead:

- **Texture programs:** an LLM writes a small recipe (palette from material properties, noise layers, edge darkening, highlights, pattern grammar) that the engine renders deterministically at 16×16. A material's look follows from its properties (rusted, polished, porous), and every texture shares the same art rules.
- **Model grammars:** blocky creatures and machines are assembled from a part library with parameters chosen from their traits.
- **Style validation:** palette, pixel density and legibility at distance are checked like any other test.

- **Sprites as text:** small sprites (flowers, items, icons) can be authored as palette-indexed text grids, which LLMs write reliably and which render pixel-exact.

Diffusion models can still help offline, proposing palettes or concept sketches that a texture program then approximates.

The base game ships its own pixel-art texture pack in a Minecraft-like style; nothing from Minecraft is required.

**Every presentation layer is replaceable.** Presentation never affects the simulation, so the server never needs to know which packs a player uses.

| Piece | First-party version ships as | Replaced by |
| --- | --- | --- |
| Texture pack | pixel-art PNGs and texture programs | any pack, layered over others |
| Shader pack | WGSL modules plus a RON schema of settings | another pack |
| Shader pack loader | a client plugin | another client plugin |
| Sky and clouds | a shader-pack stage | a pack stage or client plugin |
| Particles and ambience | RON emitter and sound-layer tables | data, or a client plugin |

Each pack declares its settings (type, range, default, and values for presets such as low, high and ultra); the settings menu is generated from them, so lighting, shadows, clouds, water, fog and the rest are user-configurable and mods can add their own. Data packs need no native code. Replacing a loader or a whole stage is a native client plugin.

## Validation, provenance and compression

Generation is easy; generating something playable, coherent, performant and stable is the hard part, so validation is a platform feature.

**The loop for every package:** generate → simulate → measure → repair → validate → publish.

| Check | Catches |
| --- | --- |
| Schema, types, references | broken data |
| Expression cost estimates | hidden performance traps |
| Permission audit | over-reaching packages |
| Compile, fuzz, determinism replay | crashes and nondeterministic WASM |
| The package's own simulation tests | intended behaviour that doesn't work |
| Standard harness | economy inflation, unreachable items, dead-end chains, ecology collapse, runaway feedback, performance budgets |
| Interaction tests with popular packages | packages valid alone but broken together |
| Bot playtests | shallow or reskinned mechanics |
| Art style checks | wrong palette or pixel density |
| LLM judges with rubrics | incoherence, clichés, poor documentation |
| Human curation and player telemetry | what is actually fun |

**Packages ship their own tests**, for example "a hive in a meadow produces at least four honey in two days" or "crops near hives grow at least 20% faster than without the package". The platform runs them alone and combined with other packages.

**Every chunk and concept has provenance:** seed, engine protocol version, generator and package versions, generated concept ids and player modifications. A chunk can answer "which generators and packages made me?", which supports debugging, migration, multiplayer, replay, long-term compatibility and AI explanation. Generated packages also record the model, brief, tool versions and any human reviewer.

**Emergence is logged.** When a metric moves in a test that shouldn't touch it, the harness records an unplanned interaction; a judge classifies it as an exploit to fix, harmless, or delightful enough to keep and hint at.

## First playable slice and what is deferred

Prove the model small before widening it: if an external mod can add a machine and a skill with no engine changes, the architecture is right.

- [ ] Engine: chunks, meshing, scheduler, action executor, one native belt kernel, a server and one client
- [ ] Frameworks: items, inventory, actions, logistics, energy, skills
- [ ] Base game: 3 ores, 6 machines, 3 skills, 2 biomes, 1 creature
- [ ] Generators: geology → climate → resources only
- [ ] One external test mod (such as beekeeping) that adds a machine and a skill without touching base code
- [ ] One generated mechanic that passes bot depth tests and the reskin detector

The base game still has to deliver a strong first hour, excellent movement and interaction, satisfying building and destruction, meaningful exploration, a deep technology loop, clear progression and memorable discoveries.

**Deferred to after 1.0, or to community mods:** markets and economy depth, professions and relationships, deep history simulation, institutions (religions, laws, sports, generated through the same game and mechanic system), politics.

**A separate project, not game design:** the platform's accounts, friends, server discovery, package registry and CDN, moderation, payments and any cross-world economy. The original three planes still apply (a control plane for accounts and discovery, a content plane for packages and assets, a realtime plane for the simulation), and servers must never mutate globally valuable ownership directly. Any blockchain-style ownership stays optional infrastructure, never a gameplay dependency.

**Open source the engine, not necessarily the network.** The runtime is open; official services for accounts, discovery, reputation, payments and moderation can be centrally governed. Code is easy to fork; a trusted network of players and creators is not. The moat is the ecosystem, not the renderer.

## Short form

If the whole philosophy had to fit on one screen:

1. Make the base game excellent; user content expands it, never excuses it.
2. Voxel world, constrained meshes, strong visual identity.
3. Seven primitives: Definitions, Traits, Fields, Actions, Processes, Agents, Generators. Roles are queries; packages ship everything.
4. Agents choose actions; actions change traits; processes evolve traits and fields; generators create.
5. Players, machines and NPCs perform the same actions; skills modify actions.
6. First-party content is built exactly like mods. Mods extend by adding, resolve by stages, override rarely.
7. Schemas live in packages; hot loops live in the engine.
8. Procgen generates causes, not decoration: every place can answer "why is this here?"
9. Reuse, parameterize, compose, subtype, then invent. Balance expansion with compression.
10. Generate decisions, not just nouns. Prove depth with bots; measure novelty by how people play.
11. Innovation is an open-ended search: LLMs mutate, evaluators select, the archive remembers stepping stones.
12. Teach one concept at a time; only the core is required.
13. LLMs author packages; the engine runs them; the world never needs an LLM to exist.
14. Validate the composed world. Record provenance for everything.
15. Deterministic, parallel, server-authoritative, cheapest-update-first simulation.
16. Keep the engine replaceable beneath a stable semantic API.
17. Everything connects: biomes, plants, food, potions, skills and weather feed each other through fields, tags and modifiers.
18. Slightly fantastic, never arbitrary; every presentation layer is a replaceable pack.

Infinite, understandable consequence, and always something new to do.
