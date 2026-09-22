import type { Update } from "@tauri-apps/plugin-updater";

export type UpdateInstallationStatus =
  | { kind: "downloading"; downloaded: number; total: number | null }
  | { kind: "installing" }
  | { kind: "ready" };

export async function installUpdate(
  update: Pick<Update, "downloadAndInstall">,
  onStatus: (status: UpdateInstallationStatus) => void,
): Promise<void> {
  let downloaded = 0;
  let total: number | null = null;
  onStatus({ kind: "downloading", downloaded, total });

  await update.downloadAndInstall((event) => {
    switch (event.event) {
      case "Started":
        downloaded = 0;
        total = event.data.contentLength ?? null;
        onStatus({ kind: "downloading", downloaded, total });
        break;
      case "Progress":
        downloaded += event.data.chunkLength;
        onStatus({ kind: "downloading", downloaded, total });
        break;
      case "Finished":
        onStatus({ kind: "installing" });
        break;
    }
  });

  onStatus({ kind: "ready" });
}
