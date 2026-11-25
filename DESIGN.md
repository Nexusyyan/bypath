# Bypath Design System (`DESIGN.md`)

Minimalist, calm, achromatic desktop instrument aesthetic aligned with Antigravity, supporting Light, Dark, and System themes.

## Brand Identity & Logo

- **Name**: `Bypath`
- **Visual Motif**: Geometric Split / Side-Channel Path (`assets/bypath-logo.svg`, `assets/bypath-banner.svg`, `apps/desktop/src/components/Icons.tsx` `BrandMark`).
- **Geometry**: Rounded charcoal tile (`#1A1A1D`, `rx="5.4"` on `24x24` viewBox) housing an untouched vertical main trunk (`#6C6C77`, `1.75px` stroke) and a highlighted active bypass side-lane curving cleanly upward into an open chevron (`#FFFFFF`, `1.8px` stroke). Designed to remain legible from `16x16` Windows system tray icons up to `512x512` GitHub branding assets.

## Color Tokens

Strictly achromatic neutral hierarchy with zero cool-blue tint, zero pure `#000000`, zero gradients, and zero flashy saturated labels.

### Light Theme (`:root`)

| Token | Value | Role |
| :--- | :--- | :--- |
| `--surface-canvas` | `#FFFFFF` | Primary main content canvas |
| `--surface-sidebar` | `#F9F9FA` | Left navigation rail, unified titlebar, subtle table headers & pagination footers |
| `--surface-subtle` | `#F7F7F9` | Inset control / search input background |
| `--surface-hover` | `#F2F2F5` | Quiet hover wash |
| `--surface-active` | `#EAEAEF` | Active navigation pill, active process tab, and secondary action fill |
| `--surface-chip` | `#F3F3F6` | Neutral inline tags (`SNI`, `TCP`, `Process Tree`, `Domain`) |
| `--ink-primary` | `#1A1A1D` | Primary text, headings, primary CTA button fill, active switches |
| `--ink-medium` | `#4A4A52` | Navigation labels and secondary data values |
| `--ink-secondary` | `#6E6E77` | Metadata, descriptions, placeholders (WCAG AA >= 4.5:1 on `#FFFFFF` / `#F9F9FA`) |
| `--line-hairline` | `rgba(0, 0, 0, 0.06)` | Default structural dividers, card outlines, table rows |
| `--line-control` | `rgba(0, 0, 0, 0.10)` | Input and secondary button outlines |
| `--line-focus` | `rgba(26, 26, 29, 0.34)` | Keyboard focus outline |
| `--status-ok` | `#27794B` | 7px indicator dot only (Running / Online / Live process tab) |
| `--status-warn` | `#9A5B13` | 7px indicator dot only (Needs Admin / Warning) |
| `--status-err` | `#B43232` | 7px indicator dot and concise blocked/error text only |
| `--status-idle` | `#A1A1AA` | 7px indicator dot (Standby / Closed) |

### Dark Theme (`[data-theme="dark"]`)

Pure charcoal neutrals with zero blue cast and crisp hairline separation:

| Token | Value | Role |
| :--- | :--- | :--- |
| `--surface-canvas` | `#161618` | Primary main content canvas |
| `--surface-sidebar` | `#111113` | Unified titlebar, sidebar chrome, table headers & footers |
| `--surface-subtle` | `#1C1C1F` | Inset controls, inputs, code snippets |
| `--surface-hover` | `#222226` | Quiet hover wash |
| `--surface-active` | `#2A2A30` | Active navigation pill, active process tab, secondary fill |
| `--surface-chip` | `#232328` | Neutral inline tags |
| `--ink-primary` | `#F4F4F5` | Primary ink, headings, primary CTA fill |
| `--ink-medium` | `#C8C8D0` | Secondary data values and nav labels |
| `--ink-secondary` | `#8E8E98` | Muted metadata and placeholders |
| `--ink-on-dark` | `#161618` | Text on primary light CTA button in dark theme |
| `--line-hairline` | `rgba(255, 255, 255, 0.07)` | Hairline structural dividers |
| `--line-control` | `rgba(255, 255, 255, 0.12)` | Control outlines |
| `--status-ok` | `#3DA86C` | 7px indicator dot (Running / Online) |
| `--status-warn` | `#D4943A` | 7px indicator dot (Warning) |
| `--status-err` | `#E05858` | 7px indicator dot and concise error text |
| `--status-idle` | `#71717A` | 7px indicator dot (Standby / Closed) |

## Geometry & Depth

- **Depth strategy**: Hairline borders only, zero default drop shadows.
- **Border radius scale**:
  - Controls (buttons, inputs, nav pills, segmented tabs, process tabs, domain chips, pagination buttons): `8px` (`--radius-sm`) and `10px` (`--radius-md`)
  - Panels, table frames, preset cards, readiness card, inline composers: `14px` (`--radius-lg`) and `16px` (`--radius-xl`)
  - Status dots, process count badges, and switches: `999px` (`--radius-pill`)
- **Spacing**: `4px` base unit (`4, 8, 12, 16, 20, 24, 32`). Unified L-frame (`44px` custom titlebar + `228px` sidebar) cradling the main canvas (`14px` top-left radius).

## Key Surface Patterns

- **Readiness Card**: Quiet onboarding card displayed at the top of the main canvas only when WinDivert driver, background service authorization, or upstream proxy needs attention; vanishes automatically once ready.
- **Telemetry Strip + Inline Sparklines**: 4-column flat readout bar with subtle monochrome 60-second SVG throughput polylines inside Upload/Download cells.
- **Process Tabs + Detail/Summary Switch + Integrated Pagination**: Persistent state across tab switches, clickable sortable headers, per-process/domain traffic aggregation view, expanded-row quick actions (Copy, Direct Bypass, Block/Unblock), and bottom `#F9F9FA` / `#111113` pagination bar.
- **Priority Rules + Dry-Run Tester**: Numbered `#1..#N` rules with up/down reordering, inline create/edit form with collapsible L4 advanced criteria, and instant Phase 1/Phase 2 dry-run match tester.
- **Live Daemon Logs & Agent DIY Card**: Filterable monochrome monospace log feed and one-click copyable `bypath mcp` / `bypath ctl --json` integration card inside Diagnostics.

## Typography & Iconography

- **Sans**: `"Segoe UI Variable Text", "Segoe UI", -apple-system, BlinkMacSystemFont, sans-serif` (`13.5px` base body, `15px / 600` section headings, `18px / 600` tabular telemetry readouts).
- **Mono**: `"Cascadia Mono", "Consolas", ui-monospace, monospace` with `font-variant-numeric: tabular-nums`, reserved strictly for PIDs, SNI hostnames, domain patterns, IP:port endpoints, file paths, logs, and byte counters.
- **Icons**: Custom monochrome `1.65px` stroke SVGs in `apps/desktop/src/components/Icons.tsx` (zero emojis or Unicode glyphs).
