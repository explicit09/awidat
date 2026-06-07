# Phase 7: Multi-user auth: Supabase Auth → existing workspace/role model (replace hardcoded LOCAL_USER_ID actor)

> **BINDING:** Read `RECONCILIATION.md` first. Where this plan conflicts with it, RECONCILIATION wins. Key: domain stays sync (D1), server crate = `crates/social-server` (D2).


**Depends on phases:** [1, 2, 4, 5]  <!-- per D7: also depends on Phase 4 (auth middleware protects Phase 4's /internal/* routes + reuses its cron-secret scheme) -->

## Prerequisites
- [USER] Create the Supabase project and enable Supabase Auth with the desired sign-in providers (email/OAuth). Phase 7 verifies tokens it does not provision the project.
- [USER] Configure the Supabase Auth redirect/callback URL the desktop sign-in flow will use (loopback or custom scheme), analogous to the existing social-provider OAuth redirect setup.
- [USER] Provide the Rust service with the Supabase project's JWT verification material as server-only secrets: either the JWKS URL (SUPABASE_URL) for asymmetric verification or SUPABASE_JWT_SECRET for HS256. These must never be bundled into the desktop app.
- [WE-STAGE] The workspace_member_roles Postgres table + RLS policy and a by-user role query (Step 3) — staged as a migration in the publishing schema.
- [WE-STAGE] The JWT-verification module, role loader, auth middleware/extractor, desktop sign-in/session commands, and the LOCAL_USER_ID removal — all code we write in this phase.
- [USER/WE] Decide whether membership administration (inviting a user to a workspace, assigning TeamRole) is in-scope UI for this phase or seeded manually in Postgres for now; the authorization logic works either way since it only reads WorkspaceMemberRole rows.

## Plan

## Goal

Replace the hardcoded single-user actor with real per-user identity from Supabase Auth, feeding the **already-tested** authorization boundary in `crates/social` (`ApiActor`/`ApiOwner` → `TeamPolicy::can_perform` → `OwnerRef`/`TeamRole`/`WorkspaceMemberRole`). The domain authorization logic is reused **unchanged**; the only new work is (a) verifying a Supabase JWT into a `user_id`, (b) loading that user's `WorkspaceMemberRole`s, and (c) constructing `ApiActor`/`ApiOwner` from that real identity at every entry point that currently calls `actor()`/`owner()` with `LOCAL_USER_ID`.

There are two call surfaces because of the server-first architecture (design lines 66-108):
- **Server (`montage-social` HTTP service)** — the authoritative entry point for OAuth, scheduling, and the cron worker. JWT verification + role loading lands here. (This is the surface that actually matters for multi-user.)
- **Desktop client (Tauri commands in `apps/desktop/src-tauri/src/commands/social.rs`)** — must stop using `LOCAL_USER_ID` and instead attach the signed-in user's Supabase access token to server calls; locally it derives the actor from the cached session.

This plan assumes Phases 1-6 have stood up the Supabase project, the Postgres publishing schema, and the `montage-social` HTTP service wrapper. Where that wrapper does not yet exist as a file, the plan notes it as a dependency and targets the equivalent boundary.

---

### What is REUSED unchanged (do not modify)
- `crates/social/src/team_service.rs` — `TeamPolicy::can_perform`, `role_allows_action`, `TeamService`. Fully tested (`team_service.rs:154-271` role-policy tests). No changes.
- `crates/social/src/model.rs` — `OwnerRef`, `TeamRole`, `TeamAction`, `WorkspaceMemberRole`, `ApiActor` inputs. No changes.
- `crates/social/src/api.rs` — `ApiActor::authorize` (`api.rs:56-62`), `authorize_read` (`api.rs:733-746`), `authorize_job_owner` (`api.rs:748-763`), and all `SocialApi::*` routes. These already take `&ApiActor`/`&ApiOwner` and are agnostic to *how* the actor was built. **No changes to authorization logic** — that is the whole point: Phase 7 only changes how the actor is *constructed*, not how it is *checked*.
- `crates/social/src/store.rs` / `crates/social/src/sqlite_store.rs` — `save_workspace_member_role` / `workspace_member_roles` (`store.rs:96-104`, `sqlite_store.rs:589-636`) already persist roles. Reused as-is for role lookup.

### What is NEWLY built
- A JWT-verification + identity-resolution module (server-side).
- A role-loading step that turns `(user_id, workspace_id)` into `Vec<WorkspaceMemberRole>` for the `ApiActor`.
- Desktop session plumbing (sign-in, cached session, token attached to requests) replacing `LOCAL_USER_ID`.
- A small workspace-membership table/source (Supabase Postgres) and the query that feeds role loading.

---

## Step 1 — Define the server-side identity/auth module

**New file:** `crates/social/src/auth_context.rs` (added to `crates/social/src/lib.rs` module list, file `crates/social/src/lib.rs`).

This is a pure, dependency-light module so it stays unit-testable and does not pull a web framework into the domain crate:

- `pub struct AuthClaims { pub user_id: String, pub email: Option<String>, pub expires_at: i64 }` — the verified subset of a Supabase Auth JWT (`sub` → `user_id`, `exp` → `expires_at`).
- `pub trait JwtVerifier { fn verify(&self, bearer: &str, now: i64) -> Result<AuthClaims, AuthContextError>; }` — abstracts verification so the domain crate has no hard dependency on a specific JWT lib and tests can inject a fake verifier (mirrors the existing `KeyProvider`/`UploadAdapter` trait-injection style already used throughout this crate, e.g. `TestKeyProvider`).
- `pub fn build_actor(claims: &AuthClaims, roles: Vec<WorkspaceMemberRole>) -> ApiActor` — thin constructor returning `ApiActor::new(claims.user_id.clone(), roles)`. This is the single seam where verified identity becomes an `ApiActor`.
- `pub fn owner_for_user(claims: &AuthClaims) -> ApiOwner` and `pub fn owner_for_workspace(workspace_id: &str) -> ApiOwner` — wrap `ApiOwner::user` / `ApiOwner::workspace`.
- `AuthContextError` (thiserror): `Missing`, `Expired`, `InvalidSignature`, `MalformedClaims`. Map all of these to `SocialApiError::Unauthorized` (extend the `From`/match in `api.rs:88-104` area if a conversion is wanted, but keep mapping to the existing `Unauthorized` variant so the redaction/HTTP-status contract from earlier phases is unchanged).

**Verification:** unit tests in the same file: a fake `JwtVerifier` that returns fixed claims; assert `build_actor` produces an `ApiActor` whose `user_id` matches `sub` and whose `workspace_roles` round-trip; assert expired/missing tokens yield the right `AuthContextError`. Run `cargo test -p montage-social auth_context`.

## Step 2 — Real JWT verification implementation (server-only, behind a feature/separate module)

**New file:** `crates/social/src/supabase_jwt.rs` (gated so the desktop build does not pull JWT crypto it does not need — add `#[cfg(feature = "server")]` and a `server` feature in `crates/social/Cargo.toml`, OR place this in the HTTP-service crate from Phase 1 if that crate exists; prefer the service crate to keep the domain crate pure).

- Implements `JwtVerifier` against Supabase. Two supported modes, decided here (resolves an open question implicitly):
  1. **Asymmetric (preferred):** fetch Supabase project JWKS (`{SUPABASE_URL}/auth/v1/.well-known/jwks.json`), verify RS256/ES256 signature, validate `iss`, `aud=authenticated`, `exp`. Cache JWKS with TTL.
  2. **Shared-secret fallback:** verify HS256 with the project JWT secret from server env (`SUPABASE_JWT_SECRET`).
- Reads config from server env only (`SUPABASE_URL`, `SUPABASE_JWT_SECRET` or JWKS URL). Never shipped to desktop.

**Verification:** integration test with a locally-minted token signed by a test key/secret (the `jsonwebtoken` crate's encode in a test); assert a valid token verifies and a tampered/expired one fails. `cargo test -p montage-social --features server supabase_jwt`.

**Dependency note:** This step depends on Phase 1 having chosen where the Rust service lives. If the service crate exists (e.g. `crates/social-server` or an `apps/` service from Phase 1), put `supabase_jwt.rs` there and have it depend on `montage-social`'s `auth_context` traits. If no service crate exists yet, this file is the first concrete server-only file and Phase 1's wrapper builds on it.

## Step 3 — Workspace-membership source + role loader

The `WorkspaceMemberRole` rows must come from somewhere authoritative in the multi-user world. Today they live in the social store (`workspace_member_roles` table, `sqlite_store.rs:589-636`). For the server, they live in Supabase Postgres.

- **Postgres (Supabase) — per D3, Phase 1 OWNS this table; Phase 7 does NOT redefine it.** The table already exists from Phase 1 in the **payload_json shape** (`workspace_id text, user_id text, payload_json text, PK(workspace_id, user_id)`) preserving the `WorkspaceMemberRole` serde round-trip. Phase 7 only **adds an RLS policy** if desired: a row readable by `auth.uid() = user_id`; the **service role** (the Rust service's connection) bypasses RLS, which is correct because the service is the trusted authorization point. Do NOT add a flat `role` column.
- **Role loader (per D3 — query the payload_json shape):** add a by-user query `pub fn workspace_member_roles_for_user(&self, user_id: &str) -> Result<Vec<WorkspaceMemberRole>, SocialStoreError>` to the **sync `SocialStore` trait** (per D1), implemented in both `PgSocialStore` and `SqliteSocialStore`. Server SQL: `SELECT payload_json FROM workspace_member_roles WHERE user_id = $1`, then deserialize each `payload_json` into `WorkspaceMemberRole` (do NOT select a `role` column — there isn't one). The existing trait method `workspace_member_roles(workspace_id)` (`store.rs:101-104`) is keyed by *workspace*; this adds the by-user variant the actor needs to authorize against any owner it targets.

**Verification:** unit test the loader against the in-memory store seeded with two workspaces for one user; assert both roles return. For Postgres, an integration test against a local Postgres (the Phase-1 test harness) seeding rows and asserting the loader returns them. `cargo test -p montage-social roles_for_user`.

## Step 4 — Wire identity into the server HTTP entry points

This is where multi-user actually takes effect. In the `montage-social` HTTP service (the axum wrapper introduced by Phase 1/5 — referenced in `commands/social.rs:6-8` as "lift onto an axum wrapper later unchanged"):

- Add an extractor/middleware that: reads `Authorization: Bearer <jwt>` → `JwtVerifier::verify` (Step 2) → `AuthClaims` → `load_roles_for_user` (Step 3) → `build_actor` (Step 1). On any failure, return 401 (mapping `AuthContextError`/`SocialApiError::Unauthorized`).
- Each route handler then calls the existing `SocialApi::*` function passing the constructed `&ApiActor` and an `&ApiOwner` derived from the request (the request specifies whether it targets the caller-as-user or a workspace; the actor's roles gate workspace access via the unchanged `TeamPolicy`).
- The cron worker entry points (`SocialApi::execute_claimed_upload_job` / `poll_upload_status`, `api.rs:644`/`api.rs:678`) take **no** `ApiActor` by design (they are system actors) — leave them unauthenticated-by-user but protect them with the service-internal auth from Phase 4 (cron secret / service role), not a Supabase user JWT. Confirm no `LOCAL_USER_ID` leaks into worker-created events (worker events already use `PublishJobActorType::Worker`/`System`, `model.rs:177-184`).

**Verification:** HTTP-level integration tests: (1) request with a valid user-A token can list user-A's accounts but a request with user-B's token gets `Unauthorized` for user-A's user-owned resources (exercises `authorize_read`/`account_owner`, `api.rs:733-763`); (2) a workspace-Publisher token can `schedule` but not `connect` (exercises `role_allows_action`, `team_service.rs:132-141`); (3) no token → 401. These mirror the existing in-crate tests (`api.rs:984-1097`, `team_service.rs:181-220`) but at the HTTP boundary.

## Step 5 — Desktop: sign-in + cached Supabase session

**New file:** `apps/desktop/src-tauri/src/commands/social_auth.rs` (registered in `apps/desktop/src-tauri/src/lib.rs` invoke_handler near the existing `commands::social::*` block, `lib.rs:274-287`).

Reuse the existing desktop OAuth/session patterns rather than inventing new ones:
- Sign-in opens the system browser to Supabase Auth (same browser-OAuth pattern already used for the social providers in `apps/desktop/src-tauri/src/publishing/oauth_listener.rs` and for ChatGPT login in `crates/auth/src/login.rs`). On callback, the desktop receives a Supabase access token + refresh token + `user.id`.
- Persist the session via the existing keychain helper (`apps/desktop/src-tauri/src/publishing/keychain.rs` `store_token`/`read_token`/`delete_token`, lines 168-201) under a new service/account key (e.g. service `montage-supabase`, kind `Session`), OR via `montage-secrets` (`apps/desktop/src-tauri/src/secrets.rs`). Reuse, do not rebuild, the secrets layer.
- Commands: `social_sign_in`, `social_sign_out` (delete cached session), `social_current_user` (return `{ userId, email }` or null). Sign-out must also clear in-memory actor state.

**Verification:** `node:assert`-style desktop tests for the TS side (Step 7) plus a Rust unit test that the session round-trips through the keychain mock. `cargo test -p <desktop crate> social_auth`.

## Step 6 — Desktop: replace `LOCAL_USER_ID` with the real session actor

**Edit:** `apps/desktop/src-tauri/src/commands/social.rs`.

- Delete `const LOCAL_USER_ID` (`social.rs:37`) and the `actor()`/`owner()` helpers (`social.rs:39-45`).
- Replace with helpers that read the cached Supabase session (Step 5): `current_claims(&state) -> Result<AuthClaims, String>` and from it `actor_for(&state) -> ApiActor` / `owner_for(&state) -> ApiOwner`. The actor's `user_id` is the Supabase `user.id`; its `workspace_roles` come from the role loader (Step 3) — for the desktop-talks-to-server architecture, the desktop simply forwards the bearer token and the **server** builds the authoritative actor (Step 4); the desktop-local actor is only for the still-local SQLite fallback path that exists today.
- Every command currently calling `actor()`/`owner()` (all of `social.rs:85-354`) and the three inline `OwnerRef::User(LOCAL_USER_ID.into())` constructions in `social_oauth_start`/`social_oauth_complete` (`social.rs:115,154,182` style) and the `created_by: LOCAL_USER_ID.into()` in `social_schedule_target` (`social.rs:294`) must use the real `user_id`. `created_by` becomes the signed-in user's id (feeds `PublishJob.created_by`, `model.rs:157`).
- When no user is signed in, these commands must return the `"unauthorized"` string (reuse `err_string`, `social.rs:54-59`) instead of silently using a fake user.
- Update the test fixtures in `social.rs:444-562` that hardcode `LOCAL_USER_ID` to use a fixed test user id; the `connected_account_never_contains_token_material` / token-safety asserts stay (`social.rs:469-562`).

**Verification:** `cargo test -p <desktop crate> social` — existing token-safety and round-trip tests must stay green with the new actor construction. Manually: signed-out → commands return unauthorized; signed-in as user-A → only user-A's accounts list.

## Step 7 — Frontend: thread session into invoke calls

**Edit:** `apps/desktop/src/campaign/publisher.ts` and the campaign/delivery UI that triggers publishing (`apps/desktop/src/shell/delivery/*`, `apps/desktop/src/campaign/store.ts`).

- Add a sign-in gate: before any `social_*` invoke, ensure `social_current_user` returns a user; otherwise route the user to sign-in. The `InvokeFn` type (`publisher.ts:24`) is unchanged — the session is read server/host-side, not passed per call — so this is mostly a UI guard plus surfacing the signed-in identity.
- Show "signed in as <email>" + sign-out in the campaign approval surface (`apps/desktop/src/campaign/CampaignApprovalPanel.tsx`).

**Verification:** existing `node:assert` frontend tests for `publisher.ts` stay green; add a test that an unauthenticated state short-circuits `startCampaignUploads` with a clear error rather than calling invoke. Run the repo's frontend test command (the `node:assert` suite for `apps/desktop/src`).

## Step 8 — Migration / backfill of the existing single-user data

The existing local SQLite store has rows owned by `OwnerRef::User("local-user")`. After Phase 7, the real user has a Supabase `user.id`.

- Provide a one-time, idempotent backfill: on first sign-in, if local rows owned by `"local-user"` exist and no rows yet exist for the real `user.id`, rewrite `owner` on `connected_accounts`, plus `created_by` on `publish_jobs`, from `local-user` → real id. This is desktop-local SQLite only (the design keeps editing/project state local, line 88-89); the server's Postgres starts empty per real user.
- Gate behind an explicit confirmation or run automatically only when exactly one local user's data exists (the solo-dev case).

**Verification:** unit test: seed store with `local-user`-owned account + job, run backfill with real id, assert `connected_accounts_for_owner(OwnerRef::User(real))` returns them and `local-user` returns none. `cargo test -p montage-social backfill_local_user` (or in the desktop crate if the migration lives there).

## Step 9 — Full workspace verification

- `make test` (`Makefile:13-14`, runs `cargo test --workspace`) must pass — confirms the unchanged `crates/social` FSM + policy tests stay green (design "testing strategy", spec lines 168-176).
- Run the desktop + frontend test suites.
- Manual end-to-end in private/sandbox mode (per spec lines 156-165, audits need not be cleared): sign in as two distinct Supabase users on two machines; confirm each sees only their own accounts/jobs and a workspace Publisher can schedule but not connect.

---

## Sequencing within the phase
Step 1 → Step 2 → Step 3 (server identity primitives) before Step 4 (wire into HTTP). Step 5 → Step 6 → Step 7 (desktop) can proceed in parallel with 1-4 once Step 1's `AuthClaims`/`build_actor` shape is fixed. Step 8 depends on Step 6. Step 9 is last.


## Open risks
- Where the server HTTP wrapper actually lives is decided in Phase 1; if no service crate exists yet, Steps 2 and 4 must create the first server-only module and the phase boundary shifts. The plan targets the documented axum wrapper referenced in commands/social.rs but that file does not yet exist in the tree.
- Membership administration (who creates workspaces and assigns roles) is unspecified by the design. Without an admin path, multi-user workspaces require manual Postgres seeding. The TeamPolicy/role model is ready; the management UX is not, and may need its own follow-up.
- Desktop dual-mode ambiguity: today commands hit local SQLite directly (commands/social.rs). The server-first design wants the desktop to be a thin client of the Rust service. Phase 7 assumes Phase 5 rewired the desktop to call the server; if Phase 5 left a local-SQLite path, the desktop must build a local actor (Step 6) AND forward a bearer token (Step 5) and the two authority sources could diverge. Need to confirm Phase 5's actual end state.
- JWT verification mode (JWKS asymmetric vs HS256 shared secret) and JWKS cache/rotation handling — Supabase has changed default signing key behavior over time; verify the project's current signing config before finalizing Step 2.
- Backfill (Step 8) of local-user-owned rows to a real user id is safe only in the single-local-user case; multi-profile desktop installs would need disambiguation. Risk of mis-assigning ownership if multiple historical owners exist.
- The YouTube 100-uploads/day-per-project cap (spec line 202-203) becomes a real multi-user constraint once multiple authenticated users share one Google project; out of scope for auth wiring but surfaces here as the first time real distinct users exist.
- Session expiry/refresh on the desktop: Supabase access tokens are short-lived; the desktop must refresh using the stored refresh token before calls or the server will 401. The refresh loop is new plumbing (Step 5) and must be tested for the offline/expired case.
