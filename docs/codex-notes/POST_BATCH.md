# Next batch: finish Nomad and Farm

Mariano's instruction on 8 October 2026: "after this batch of work, focus on getting the first 2 ages (nomad etc) instead of the whole game".

The design's first two ages are Nomad (levels 1–10) and Farm (levels 5–20). The levels overlap intentionally. Nomad exits through the Homestead Hearth; Farm exits through the Crucible Kiln. The immediate goal is a polished playable path through both, not building the later-age content listed in the design.

Finish the recovered batch already underway, including its review and gate fixes. The following responsibilities replace the old open-ended follow-up queues. Start by checking the actual game and existing data, then fix concrete blockers in small phases. Coordinate shared interfaces through the workstream status files; don't implement the same system twice.

| Workstream | Next focus after the recovered phase |
| --- | --- |
| Gameplay | Own the early progression gap list and the main path: real recipes, resources, tools, Hearth, pottery/fuel/copper/Kiln, quest rewards and age transitions. Fix blocking missing mechanics and requirements before side quests. Keep canonical-name/ownership fixes where needed for saves and animals. |
| UI | First-session guidance, Look It Up, readable recipe requirements and sources, quest tracking, item search, health/combat feedback, and English/Japanese clarity through both ages. |
| Combat | Early enemies and weapons, understandable telegraphs, viable guard/dodge, food and healing, first-night safety, death/respawn, and early encounter balance. Later weapon tiers and combat breadth wait. |
| Creatures | Reliable early animal access, favourite foods, taming, keeping, breeding, animal condition/products and first-night spawning. Verify the resources and animal routes used by the early quests. |
| World | Accessible early wood/stone/flint/clay/sand/coal/native copper, food, water and usable home sites across representative seeds. Fix generation problems that block or frustrate early progression; later-tier ore expansion waits. |
| Settlements | Useful, readable early hamlets/villages and safe villager interactions. Only add trade or services when they make a promised Nomad/Farm route work. Broad P59 clothing/behaviour/festival expansion waits. |
| Towns | Finish current P57a, then focus on early exploration and settlement discoverability, bounded generation cost and relevant progression sources. P57b/P57c large-town/city expansion waits. |
| Homes | Complete current P58a, then prioritise functional early bedding, shelter, Hearth and Farm home requirements, sensible furnishing recipes and usable entrances. Decorative catalogues and broad interior generation wait. |
| Integration-quality | Own fresh-survival acceptance through both ages, client/server/save/respawn verification, representative-seed evidence, performance and the full-gated play release. Audit actual early-age gate-machine behaviour before queuing more industrial machinery. |

Acceptance: an ordinary player can start a fresh survival world, discover and complete the Nomad main path, establish a Hearth and reach Farm, sustain a small home/farm, complete the Farm main path and light the Crucible Kiln. No debug-given materials, skipped prerequisites, hidden dead-end recipes, broken quests, save/reload regressions or untested release builds. The first main path comes before implementing every optional quest; supported routes must work as described.
