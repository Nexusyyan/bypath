import { describe, expect, it } from "vitest";
import {
  IDLE_UPDATE_PROGRESS,
  canAutoInstallReport,
  clampPercent,
  isUpdateBusy,
  isUpdateDownloadingOrStaging,
  isUpdateReadyForReport,
} from "./update";
import type { StagedUpdateInfo, UpdateCheckReport, UpdateProgressEvent } from "./types";

const sampleReport: UpdateCheckReport = {
  current_version: "0.4.2",
  latest_version: "0.4.3",
  latest_tag: "v0.4.3",
  update_available: true,
  release_name: "Bypath v0.4.3",
  release_url: "https://github.com/Nexusyyan/bypath/releases/tag/v0.4.3",
  published_at: "2026-11-26T08:00:00Z",
  release_notes: "- Seamless in-app updater",
  primary_asset_name: "bypath-v0.4.3-windows-x64.zip",
  primary_asset_url:
    "https://github.com/Nexusyyan/bypath/releases/download/v0.4.3/bypath-v0.4.3-windows-x64.zip",
  primary_asset_size: 8_400_000,
  nodriver_asset_name: "bypath-v0.4.3-windows-x64-nodriver.zip",
  nodriver_asset_url:
    "https://github.com/Nexusyyan/bypath/releases/download/v0.4.3/bypath-v0.4.3-windows-x64-nodriver.zip",
  nodriver_asset_size: 8_100_000,
  checksums_asset_url:
    "https://github.com/Nexusyyan/bypath/releases/download/v0.4.3/SHA256SUMS.txt",
  checked_via: "proxy:local_socks",
  checked_at_ms: 1_700_000_000_000,
};

const sampleStaged: StagedUpdateInfo = {
  target_version: "0.4.3",
  target_tag: "v0.4.3",
  asset_name: "bypath-v0.4.3-windows-x64-nodriver.zip",
  sha256: "a".repeat(64),
  downloaded_via: "proxy:local_socks",
  staged_at_ms: 1_700_000_010_000,
};

describe("in-app update state helpers", () => {
  it("classifies busy and staging stages accurately", () => {
    expect(isUpdateBusy("idle")).toBe(false);
    expect(isUpdateBusy("ready")).toBe(false);
    expect(isUpdateBusy("error")).toBe(false);
    expect(isUpdateBusy("downloading")).toBe(true);
    expect(isUpdateBusy("verifying")).toBe(true);
    expect(isUpdateBusy("extracting")).toBe(true);
    expect(isUpdateBusy("applying")).toBe(true);

    expect(isUpdateDownloadingOrStaging("downloading")).toBe(true);
    expect(isUpdateDownloadingOrStaging("verifying")).toBe(true);
    expect(isUpdateDownloadingOrStaging("extracting")).toBe(true);
    expect(isUpdateDownloadingOrStaging("applying")).toBe(false);
  });

  it("checks whether a report can be auto-installed safely", () => {
    expect(canAutoInstallReport(sampleReport)).toBe(true);
    expect(canAutoInstallReport({ ...sampleReport, update_available: false })).toBe(false);
    expect(canAutoInstallReport({ ...sampleReport, checksums_asset_url: null })).toBe(false);
    expect(canAutoInstallReport(null)).toBe(false);
  });

  it("matches staged ready state against active release report", () => {
    const readyProgress: UpdateProgressEvent = {
      ...IDLE_UPDATE_PROGRESS,
      stage: "ready",
      version: "0.4.3",
      tag: "v0.4.3",
      percent: 100,
    };
    expect(isUpdateReadyForReport(readyProgress, sampleStaged, sampleReport)).toBe(true);
    expect(isUpdateReadyForReport(readyProgress, sampleStaged, null)).toBe(true);
    expect(
      isUpdateReadyForReport(readyProgress, { ...sampleStaged, target_tag: "v0.4.2" }, sampleReport)
    ).toBe(false);
    expect(isUpdateReadyForReport(IDLE_UPDATE_PROGRESS, sampleStaged, sampleReport)).toBe(false);
  });

  it("clamps progress percentages cleanly", () => {
    expect(clampPercent(-5)).toBe(0);
    expect(clampPercent(48.6)).toBe(49);
    expect(clampPercent(150)).toBe(100);
    expect(clampPercent(Number.NaN)).toBe(0);
  });
});
