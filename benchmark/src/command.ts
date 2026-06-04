import { spawn } from "node:child_process";

import type { CommandResult } from "./types.ts";

const OUTPUT_LIMIT_BYTES = 2 * 1024 * 1024;

export interface RunCommandOptions {
  cwd: string;
  env?: NodeJS.ProcessEnv;
  timeoutMs?: number;
  secrets?: string[];
  shell?: boolean;
}

export function redactText(text: string, secrets: string[] = []): string {
  let redacted = text;
  for (const secret of secrets) {
    if (secret) {
      redacted = redacted.split(secret).join("[REDACTED]");
    }
  }
  return redacted;
}

export function redactEnv(env: NodeJS.ProcessEnv): NodeJS.ProcessEnv {
  const redacted = { ...env };
  for (const key of Object.keys(redacted)) {
    if (/key|token|secret|password/i.test(key) && redacted[key]) {
      redacted[key] = "[REDACTED]";
    }
  }
  return redacted;
}

export async function runShellCommand(command: string, options: RunCommandOptions): Promise<CommandResult> {
  return runProcess(command, [], {
    ...options,
    shell: true,
  });
}

export async function runProcess(command: string, args: string[], options: RunCommandOptions): Promise<CommandResult> {
  const startedAtMs = Date.now();
  const startedAt = new Date(startedAtMs).toISOString();
  const secrets = options.secrets ?? [];
  let stdout = "";
  let stderr = "";
  let stdoutBytes = 0;
  let stderrBytes = 0;
  let timedOut = false;

  return new Promise((resolve) => {
    const child = spawn(command, args, {
      cwd: options.cwd,
      env: {
        ...process.env,
        ...(options.env ?? {}),
      },
      shell: options.shell ?? false,
    });

    const timer = options.timeoutMs
      ? setTimeout(() => {
          timedOut = true;
          child.kill("SIGTERM");
          setTimeout(() => {
            if (!child.killed) child.kill("SIGKILL");
          }, 5_000).unref();
        }, options.timeoutMs)
      : null;

    child.stdout?.on("data", (chunk: Buffer) => {
      const next = chunk.toString("utf8");
      stdoutBytes += Buffer.byteLength(next);
      if (stdoutBytes <= OUTPUT_LIMIT_BYTES) {
        stdout += next;
      }
    });

    child.stderr?.on("data", (chunk: Buffer) => {
      const next = chunk.toString("utf8");
      stderrBytes += Buffer.byteLength(next);
      if (stderrBytes <= OUTPUT_LIMIT_BYTES) {
        stderr += next;
      }
    });

    child.on("error", (error) => {
      if (timer) clearTimeout(timer);
      const finishedAtMs = Date.now();
      resolve({
        command,
        args,
        exitCode: null,
        stdout: redactText(appendTruncation(stdout, stdoutBytes), secrets),
        stderr: redactText(appendTruncation(`${stderr}${error.message}\n`, stderrBytes), secrets),
        startedAt,
        finishedAt: new Date(finishedAtMs).toISOString(),
        durationMs: finishedAtMs - startedAtMs,
        timedOut,
        error: error.message,
      });
    });

    child.on("close", (exitCode) => {
      if (timer) clearTimeout(timer);
      const finishedAtMs = Date.now();
      resolve({
        command,
        args,
        exitCode,
        stdout: redactText(appendTruncation(stdout, stdoutBytes), secrets),
        stderr: redactText(appendTruncation(stderr, stderrBytes), secrets),
        startedAt,
        finishedAt: new Date(finishedAtMs).toISOString(),
        durationMs: finishedAtMs - startedAtMs,
        timedOut,
      });
    });
  });
}

export function formatCommand(command: string, args: string[]): string {
  return [command, ...args].map(shellQuote).join(" ");
}

function shellQuote(value: string): string {
  if (/^[A-Za-z0-9_./:=@+-]+$/.test(value)) {
    return value;
  }
  return `'${value.replace(/'/g, "'\\''")}'`;
}

function appendTruncation(value: string, bytes: number): string {
  if (bytes <= OUTPUT_LIMIT_BYTES) return value;
  return `${value}\n[output truncated after ${OUTPUT_LIMIT_BYTES} bytes]\n`;
}
