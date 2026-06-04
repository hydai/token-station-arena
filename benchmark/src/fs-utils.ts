import { constants } from "node:fs";
import * as fs from "node:fs/promises";
import path from "node:path";

export async function pathExists(filePath: string): Promise<boolean> {
  try {
    await fs.access(filePath, constants.F_OK);
    return true;
  } catch {
    return false;
  }
}

export async function readText(filePath: string): Promise<string> {
  return fs.readFile(filePath, "utf8");
}

export async function readJson<T>(filePath: string): Promise<T> {
  return JSON.parse(await readText(filePath)) as T;
}

export async function writeText(filePath: string, content: string): Promise<void> {
  await fs.mkdir(path.dirname(filePath), { recursive: true });
  await fs.writeFile(filePath, content, "utf8");
}

export async function writeJson(filePath: string, value: unknown): Promise<void> {
  await writeText(filePath, `${JSON.stringify(value, null, 2)}\n`);
}

export async function ensureDir(dirPath: string): Promise<void> {
  await fs.mkdir(dirPath, { recursive: true });
}

export async function copyDir(source: string, destination: string): Promise<void> {
  await fs.cp(source, destination, {
    recursive: true,
    force: false,
    errorOnExist: true,
  });
}

export async function listDirectories(dirPath: string): Promise<string[]> {
  const entries = await fs.readdir(dirPath, { withFileTypes: true });
  return entries.filter((entry) => entry.isDirectory()).map((entry) => path.join(dirPath, entry.name));
}

export async function findFiles(dirPath: string, fileName: string): Promise<string[]> {
  const matches: string[] = [];
  const entries = await fs.readdir(dirPath, { withFileTypes: true });

  for (const entry of entries) {
    const fullPath = path.join(dirPath, entry.name);
    if (entry.isDirectory()) {
      matches.push(...(await findFiles(fullPath, fileName)));
    } else if (entry.isFile() && entry.name === fileName) {
      matches.push(fullPath);
    }
  }

  return matches;
}

export function relativeArtifact(runDir: string, artifactPath: string): string {
  return path.relative(runDir, artifactPath).split(path.sep).join("/");
}
