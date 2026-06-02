# Repo workflow — FileWisely + UCE Sidekick (two repos)

Two separate local clones. Same Supabase production project; different git repos and deploy paths.

```bash
git clone https://github.com/jeffatkins-filewisely/filewisely.git
git clone https://github.com/jeffatkins-filewisely/uce.git
```

| Repo | What it controls |
|------|------------------|
| **filewisely** | Main Lovable app, Supabase edge functions, migrations, Theo, portals |
| **uce** | Sidekick desktop app, Tauri/Rust, MSI releases, Live Mirror writer |

## Main production build → open `filewisely` in Cursor

```bash
cd filewisely
supabase login
supabase link --project-ref pujwbzqnoevqxrwipnwo
```

That connects Cursor to the same production Supabase backend Lovable uses.

### Use Cursor in **filewisely** for

- Theo
- Supabase functions (`supabase/functions/**`)
- Contracts (`_shared/contracts.ts`, Zod, etc.)
- `ccc-package-claim-batch`, `ccc-package-ack`, `uce-ingest`
- Migrations (`supabase/migrations/**`)

Deploy without a Lovable frontend rebuild:

```bash
supabase functions deploy ccc-package-claim-batch
supabase functions deploy ccc-package-ack
supabase functions deploy uce-ingest
```

### Use Cursor in **uce** for

- Sidekick desktop app (`src-tauri/**`, `src/**`)
- Tauri/Rust
- CCC Import mirror writer (`src-tauri/src/ccc_package_sync.rs`)
- MSI releases (git tags → GitHub Actions)
- Version bumps (`package.json`, `Cargo.toml`, `tauri.conf.json`)

See `docs/CCC_PACKAGE_SYNC.md`, `docs/CONTRACTS.md`.

## Important rule

**Edge/backend changes happen in `filewisely`. Desktop changes happen in `uce`.**

The `backend/` folder inside **uce** is a **mirror/reference** only — not the production source of truth. After edge contract changes in **filewisely**, sync into **uce**:

- `docs/contracts/*.schema.json`
- `backend/contracts/*.ts`
- Rust preflight: `src-tauri/src/api_contracts.rs` (uce only)

## Every session

**filewisely:**

```bash
git pull origin main
# edit …
git commit -m "…"
git push origin main
```

Lovable picks up pushes on its next turn.

**uce:**

```bash
git pull origin main
# edit …
git tag v0.1.xx && git push origin v0.1.xx   # when shipping MSI
```

## Cursor lane rules

- **filewisely:** copy `docs/cursorrules-filewisely.template` → `.cursorrules` in that repo root
- **uce:** `.cursorrules` in this repo (desktop lane)

## Prompt hygiene

- Name exact paths: `supabase/functions/ccc-package-ack/index.ts`
- Review diffs — reject files outside the requested lane
