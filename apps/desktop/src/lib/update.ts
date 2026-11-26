import type {
  StagedUpdateInfo,
  UpdateCheckReport,
  UpdateProgressEvent,
  UpdateStage,
} from "./types";

export const IDLE_UPDATE_PROGRESS: UpdateProgressEvent = {
  stage: "idle",
  version: "",
  tag: "",
  asset_name: null,
  downloaded_bytes: 0,
  total_bytes: null,
  percent: 0,
  route: null,
  error: null,
};

export function isUpdateBusy(stage: UpdateStage): boolean {
  return (
    stage === "downloading" ||
    stage === "verifying" ||
    stage === "extracting" ||
    stage === "applying"
  );
}

export function isUpdateDownloadingOrStaging(stage: UpdateStage): boolean {
  return stage === "downloading" || stage === "verifying" || stage === "extracting";
}

export function isUpdateReadyForReport(
  progress: UpdateProgressEvent,
  staged: StagedUpdateInfo | null,
  report: UpdateCheckReport | null
): boolean {
  if (progress.stage !== "ready" || !staged) {
    return false;
  }
  if (!report) {
    return true;
  }
  return staged.target_tag === report.latest_tag;
}

export function canAutoInstallReport(report: UpdateCheckReport | null): boolean {
  if (!report || !report.update_available) {
    return false;
  }
  const hasArtifact = Boolean(
    report.nodriver_asset_url || report.primary_asset_url || report.gui_exe_asset_url
  );
  return hasArtifact && Boolean(report.checksums_asset_url);
}

export function clampPercent(percent: number): number {
  if (!Number.isFinite(percent)) return 0;
  return Math.max(0, Math.min(100, Math.round(percent)));
}
