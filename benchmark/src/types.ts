export type CompletionStatus = "passed" | "partial" | "failed" | "timeout" | "error";

export interface ModelConfig {
  id: string;
  displayName: string;
  provider: string;
  model: string;
  claudeModelStrategy: string;
  enabled: boolean;
}

export interface CheckConfig {
  name: string;
  command: string;
  timeoutSeconds?: number;
}

export interface TaskConfig {
  id: string;
  title: string;
  description: string;
  fixturePath: string;
  promptFile: string;
  setup: string[];
  checks: CheckConfig[];
  success: {
    requiredChecks: string[];
  };
  judge?: {
    rubric?: Record<string, string>;
    unrelatedChangePolicy?: string;
    allowedChangePaths?: string[];
  };
  expectedFiles?: string[];
}

export interface BenchmarkConfig {
  runsPerTaskModel: number;
  timeoutSeconds: number;
  outputDir: string;
  reportDir: string;
  claude: {
    baseUrl: string;
    outputFormat: string;
    projectSettingsFile: string;
    disableExperimentalBetas: boolean;
  };
  tokenStation: {
    enabled: boolean;
    mode: string;
    correlation: string;
    dumpPath: string;
    matchWindowPaddingSeconds?: number;
  };
  judge: {
    enabled: boolean;
    provider: string;
    model: string;
    minimumScore: number;
  };
  article: {
    title: string;
    outputFile: string;
  };
}

export interface LoadedTask {
  config: TaskConfig;
  taskDir: string;
  fixtureDir: string;
  promptPath: string;
  prompt: string;
}

export interface CommandResult {
  command: string;
  args?: string[];
  exitCode: number | null;
  stdout: string;
  stderr: string;
  startedAt: string;
  finishedAt: string;
  durationMs: number;
  timedOut: boolean;
  error?: string;
}

export interface CheckResult {
  name: string;
  command: string;
  exitCode: number | null;
  passed: boolean;
  durationMs: number;
  stdoutPath?: string;
  stderrPath?: string;
  timedOut?: boolean;
}

export interface JudgeFinding {
  severity: "minor" | "major" | "critical" | string;
  category: string;
  message: string;
}

export interface JudgeResult {
  enabled: boolean;
  modelId: string;
  score: number | null;
  passed: boolean;
  correctness: number | null;
  maintainability: number | null;
  scopeControl: number | null;
  hasUnrelatedChanges: boolean;
  findings: JudgeFinding[];
  rawOutputPath?: string;
  error?: string;
}

export interface TokenUsage {
  source: string;
  correlation: string;
  dumpFile: string;
  input: number | null;
  output: number | null;
  cacheCreationInput: number | null;
  cacheReadInput: number | null;
  total: number | null;
  estimatedCostUsd: number | null;
}

export interface RunResult {
  runId: string;
  taskId: string;
  modelId: string;
  providerModelId: string;
  runIndex: number;
  provider: string;
  startedAt: string;
  finishedAt: string;
  durationMs: number;
  claudeExitCode: number | null;
  claude: {
    sessionId: string | null;
    outputFormat: string;
    commandStrategy: string[];
  };
  checks: CheckResult[];
  completion: {
    status: CompletionStatus;
    reason: string;
  };
  tokens: TokenUsage | null;
  judge: JudgeResult;
  artifacts: {
    stdout: string;
    stderr: string;
    claudeOutput: string;
    diff: string;
    workspace: string;
    checks: string;
    modelConfig: string;
  };
  changedFiles: string[];
  warnings: string[];
  humanAudit: {
    requiredForMvp: boolean;
    score: number | null;
    notes: string;
  };
}

export interface CliOptions {
  tasks?: string;
  models?: string;
  runs?: number;
  timeout?: number;
  tokenDump?: string;
  skipJudge?: boolean;
  skipArticle?: boolean;
  dryRun?: boolean;
  output?: string;
  input?: string;
  runId?: string;
  runsDir?: string;
}
