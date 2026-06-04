import path from "node:path";

import { runShellCommand } from "./command.ts";
import { ensureDir, writeText } from "./fs-utils.ts";
import type { CheckResult, CompletionStatus, JudgeResult, LoadedTask } from "./types.ts";

export interface EvaluationResult {
  checks: CheckResult[];
  status: CompletionStatus;
  reason: string;
}

export async function runChecks(params: {
  task: LoadedTask;
  workspaceDir: string;
  runDir: string;
  timeoutMs: number;
  secrets?: string[];
}): Promise<CheckResult[]> {
  const checksDir = path.join(params.runDir, "checks");
  await ensureDir(checksDir);
  const results: CheckResult[] = [];

  for (const check of params.task.config.checks) {
    const result = await runShellCommand(check.command, {
      cwd: params.workspaceDir,
      timeoutMs: (check.timeoutSeconds ?? Math.ceil(params.timeoutMs / 1000)) * 1000,
      secrets: params.secrets,
    });
    const stdoutPath = path.join(checksDir, `${check.name}.stdout.txt`);
    const stderrPath = path.join(checksDir, `${check.name}.stderr.txt`);
    await writeText(stdoutPath, result.stdout);
    await writeText(stderrPath, result.stderr);

    results.push({
      name: check.name,
      command: check.command,
      exitCode: result.exitCode,
      passed: result.exitCode === 0 && !result.timedOut,
      durationMs: result.durationMs,
      stdoutPath: `checks/${check.name}.stdout.txt`,
      stderrPath: `checks/${check.name}.stderr.txt`,
      timedOut: result.timedOut,
    });
  }

  return results;
}

export function classifyCompletion(params: {
  task: LoadedTask;
  checks: CheckResult[];
  judge: JudgeResult;
  claudeExitCode: number | null;
  claudeTimedOut: boolean;
  changedFiles: string[];
  infrastructureError?: string;
}): { status: CompletionStatus; reason: string } {
  if (params.infrastructureError) {
    return {
      status: "error",
      reason: params.infrastructureError,
    };
  }

  if (params.claudeTimedOut) {
    return {
      status: "timeout",
      reason: "Claude execution exceeded the configured timeout.",
    };
  }

  const requiredChecks = new Set(params.task.config.success.requiredChecks);
  const failedRequiredChecks = params.checks.filter((check) => requiredChecks.has(check.name) && !check.passed);

  if (failedRequiredChecks.length === 0 && params.judge.enabled && !params.judge.passed) {
    return {
      status: "partial",
      reason: `Required checks passed, but judge failed: ${judgeFailureReason(params.judge)}`,
    };
  }

  if (failedRequiredChecks.length === 0) {
    return {
      status: "passed",
      reason: params.judge.enabled
        ? "All required checks passed and judge accepted the run."
        : "All required checks passed; judge was skipped.",
    };
  }

  if (params.changedFiles.length > 0 || params.checks.some((check) => check.passed)) {
    return {
      status: "partial",
      reason: `Failed required check(s): ${failedRequiredChecks.map((check) => check.name).join(", ")}.`,
    };
  }

  if (params.claudeExitCode !== 0) {
    return {
      status: "failed",
      reason: `Claude exited with code ${params.claudeExitCode}; no usable change was produced.`,
    };
  }

  return {
    status: "failed",
    reason: `No usable change was produced; failed required check(s): ${failedRequiredChecks
      .map((check) => check.name)
      .join(", ")}.`,
  };
}

function judgeFailureReason(judge: JudgeResult): string {
  if (judge.error) return judge.error;
  if (judge.hasUnrelatedChanges) return "unrelated changes detected";
  if (judge.score !== null) return `score ${judge.score}`;
  return "judge did not return a passing result";
}
