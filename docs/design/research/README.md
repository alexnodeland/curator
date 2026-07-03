# Design research archive

Raw inputs to the v1 design round, archived verbatim-in-structure for provenance:

| file | what it is |
|---|---|
| `designs.json` | the three parallel design proposals (distribution, integration fabric, MCP/agent architecture) |
| `verdicts.json` | the adversarial verdict passes run against those designs — findings, must-fix rulings, cut lists. [decisions.md](../decisions.md) distills these |
| `roadmap.json` | the phased build plan as drafted pre-adaptation. [roadmap.md](../roadmap.md) is the adapted, current version |
| `kpState.json` | state-of-the-world survey of the Knowledge Plane effort at design time |
| `curioContract.json` | consumer-side review notes on Curio's contracts (frontmatter/v1, events/v1) |
| `ecosystem.json` | survey of the surrounding tool ecosystem (readers, reference managers, PKM tools) |
| `distBaseline.json` | distribution/packaging baseline survey |

Two things to know when reading these:

1. **Sanitized.** These documents were drafted alongside a private reference
   deployment. Before archiving, identifiers of that deployment (hostnames, LAN
   addresses, specific third-party service names used only in that instance) were
   mechanically replaced with role-named placeholders (`llm-gateway`,
   `git-forge`, `push-notifier`, `secrets-manager`, `192.0.2.x`, …). The design
   content is untouched; only instance identifiers were substituted. This mirrors a
   core product rule: the product knows deployment *roles*, never a particular
   private instance.
2. **Superseded in places.** `roadmap.json` predates the final language decision
   (it describes a Python workspace; the build is Rust — see
   [decisions.md §6](../decisions.md)) and uses pre-contract tool names. Where these
   files disagree with the published contracts or with
   [architecture.md](../architecture.md), the contracts win.
