# Curated technology icons

Harpoon bundles only the 12 Simple Icons path shapes used by the container technology resolver, in `ui/harpoon-desktop/src/lib/technologyIcons.js`. The UI does not ship the `simple-icons` package, the full catalog, or any runtime CDN request.

## Provenance and rights

- Catalog and metadata checked: [Simple Icons `simple-icons.json`](https://raw.githubusercontent.com/simple-icons/simple-icons/develop/data/simple-icons.json), current catalog observed at 3,453 icons on 2026-10-16.
- SVG geometry checked from the corresponding official package assets at `https://simpleicons.org/icons/{slug}.svg`.
- Simple Icons project license: [CC0-1.0](https://github.com/simple-icons/simple-icons/blob/develop/LICENSE.md).
- Simple Icons' [legal disclaimer](https://github.com/simple-icons/simple-icons/blob/develop/DISCLAIMER.md) states that project CC0 does not waive trademark rights, that per-icon license data can be absent or outdated, and that users must obtain any permissions required for their use.
- The current metadata audit found no per-icon `license` field for these 12 entries. Harpoon therefore makes no claim that a brand owner has granted trademark permission; the marks are shown only as technology-identification UI and remain subject to each owner’s guidance.

| Harpoon key | Simple Icons title / slug | Brand hex | Source / guidance check |
|---|---|---:|---|
| `postgres` | PostgreSQL / `postgresql` | `#4169E1` | [logo](https://wiki.postgresql.org/wiki/Logo) · [trademarks](https://www.postgresql.org/about/policies/trademarks/) |
| `redis` | Redis / `redis` | `#FF4438` | [brand guidelines](https://redis.io/brand-guidelines) |
| `nginx` | NGINX / `nginx` | `#009639` | [press assets](https://www.nginx.com/press/) |
| `node` | Node.js / `nodedotjs` | `#5FA04E` | [branding](https://nodejs.org/en/about/branding) |
| `mysql` | MySQL / `mysql` | `#4479A1` | [MySQL legal/logo resources](https://www.mysql.com/about/legal/logos.html) |
| `mariadb` | MariaDB / `mariadb` | `#003545` | [MariaDB logos](https://mariadb.com/about-us/logos/) |
| `mongo` | MongoDB / `mongodb` | `#47A248` | [brand resources](https://www.mongodb.com/brand-resources) |
| `rabbitmq` | RabbitMQ / `rabbitmq` | `#FF6600` | [logo resources](https://www.rabbitmq.com/resources/logos) |
| `traefik` | Traefik Proxy / `traefikproxy` | `#24A1C1` | [Traefik brand](https://traefik.io/brand) |
| `caddy` | Caddy / `caddy` | `#1F88C9` | [Caddy resources](https://caddyserver.com/resources) |
| `python` | Python / `python` | `#3776AB` | [Python logos](https://www.python.org/community/logos/) |
| `alpine` | Alpine Linux / `alpinelinux` | `#0D597F` | [Alpine Linux](https://alpinelinux.org/) |

MariaDB and Alpine use a documented display-color lift (`#3DA6C1` and `#4EA7D1`) because their canonical dark colors are below the icon contrast target on Harpoon graphite. Geometry and source brand colors remain unchanged in the provenance table. Unknown technologies use the existing Phosphor `ImageSquare` fallback.
