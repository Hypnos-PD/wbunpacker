import {useEffect, useState} from "react";
import {invoke} from "@tauri-apps/api/core";
import {listen} from "@tauri-apps/api/event";

type Check = {
  name: string;
  status: "ok" | "warn" | "error";
  message: string;
};

type DoctorReport = {
  ok: boolean;
  config: string;
  checks: Check[];
};

type ValidationReport = {
  ok: boolean;
  file: string;
  operator?: string;
  issues: string[];
};

type CredentialSummary = {
  operator: string;
  kind: string;
  fingerprint: string;
  valid: boolean;
  issues: string[];
};

type RuntimeProbe = {
  subject_type: string;
  subject_id: string;
  tested_at: string;
  request_scope: string;
  status: "success" | "failed";
  resource_version?: string;
  app_version?: string;
  resource_version_unknown: boolean;
  error?: string;
};

type PublishResult = {
  profile_id: string;
  operator: string;
  created: boolean;
};

type DetectionResult = {
  profile_id: string;
  profile_path: string;
};

type PipelineStep = {
  id: string;
  group: string;
  description: string;
  command: string[];
  cwd: string;
  depends_on: string[];
};

type PipelineCommandResult = {
  success: boolean;
  code: number | null;
};

type PipelineOutputEvent = {
  stream: "stdout" | "stderr";
  line: string;
};

export default function App() {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [doctor, setDoctor] = useState<DoctorReport | null>(null);
  const [profilePath, setProfilePath] = useState(
    "contracts/examples/runtime-profile.cygames.json"
  );
  const [profile, setProfile] = useState<ValidationReport | null>(null);
  const [detectOperator, setDetectOperator] = useState("cygames");
  const [gameRoot, setGameRoot] = useState("");
  const [dotnetPath, setDotnetPath] = useState("dotnet");
  const [il2cppDumperDir, setIl2cppDumperDir] = useState("");
  const [detected, setDetected] = useState<DetectionResult | null>(null);
  const [operator, setOperator] = useState("cygames");
  const [credentialPath, setCredentialPath] = useState("");
  const [credential, setCredential] = useState<CredentialSummary | null>(null);
  const [apiBase, setApiBase] = useState("https://sva.hypd.asia");
  const [tokenPath, setTokenPath] = useState("");
  const [probe, setProbe] = useState<RuntimeProbe | null>(null);
  const [published, setPublished] = useState<PublishResult | null>(null);
  const [pipelineFile, setPipelineFile] = useState("pipeline.toml");
  const [svwbExecutable, setSvwbExecutable] = useState("svwb");
  const [pipelineSteps, setPipelineSteps] = useState<PipelineStep[]>([]);
  const [selectedSteps, setSelectedSteps] = useState<string[]>([]);
  const [pipelineLog, setPipelineLog] = useState("");
  const [pipelineResult, setPipelineResult] = useState("");

  useEffect(() => {
    let unlisten: (() => void) | undefined;
    void listen<PipelineOutputEvent>("pipeline-output", event => {
      setPipelineLog(current => `${current}${event.payload.stream === "stderr" ? "! " : ""}${event.payload.line}\n`);
    }).then(dispose => {
      unlisten = dispose;
    });
    return () => unlisten?.();
  }, []);

  async function run(action: () => Promise<void>) {
    setBusy(true);
    setError("");
    try {
      await action();
    } catch (cause) {
      setError(String(cause));
    } finally {
      setBusy(false);
    }
  }

  async function loadPipelineSteps() {
    await run(async () => {
      const steps = await invoke<PipelineStep[]>("pipeline_list", {
        file: pipelineFile,
        executable: svwbExecutable || null
      });
      setPipelineSteps(steps);
      setSelectedSteps(current => current.filter(id => steps.some(step => step.id === id)));
      setPipelineResult(`已读取 ${steps.length} 个步骤`);
    });
  }

  async function checkPipeline() {
    await run(async () => setPipelineResult(
      await invoke<string>("pipeline_check", {
        file: pipelineFile,
        executable: svwbExecutable || null
      })
    ));
  }

  async function executePipeline(dryRun: boolean) {
    await run(async () => {
      setPipelineLog("");
      setPipelineResult(dryRun ? "Dry Run 执行中" : "管线执行中");
      const result = await invoke<PipelineCommandResult>("pipeline_run", {
        file: pipelineFile,
        executable: svwbExecutable || null,
        only: selectedSteps,
        dryRun,
        continueOnError: false
      });
      setPipelineResult(result.success
        ? (dryRun ? "Dry Run 完成" : "管线执行完成")
        : `执行失败，退出码 ${result.code ?? "unknown"}`);
    });
  }

  function toggleStep(id: string) {
    setSelectedSteps(current => current.includes(id)
      ? current.filter(value => value !== id)
      : [...current, id]);
  }

  return (
    <main>
      <header>
        <div>
          <span className="eyebrow">SVWB</span>
          <h1>Toolkit</h1>
        </div>
        <span className={busy ? "state busy" : "state"}>
          {busy ? "运行中" : "空闲"}
        </span>
      </header>

      {error && (
        <p className="error" role="alert">
          {error}
        </p>
      )}

      <section className="panel">
        <div className="panel-title">
          <h2>数据管线</h2>
          <span className="meta">直接调用 svwb pipeline</span>
        </div>
        <div className="row">
          <label className="grow">
            pipeline.toml
            <input value={pipelineFile} onChange={event => setPipelineFile(event.target.value)} />
          </label>
          <label>
            svwb
            <input value={svwbExecutable} onChange={event => setSvwbExecutable(event.target.value)} />
          </label>
        </div>
        <div className="row">
          <button disabled={busy || !pipelineFile} onClick={() => void checkPipeline()}>
            校验清单
          </button>
          <button disabled={busy || !pipelineFile} onClick={() => void loadPipelineSteps()}>
            读取步骤
          </button>
          <button
            disabled={busy || selectedSteps.length === 0}
            onClick={() => void executePipeline(true)}
          >
            Dry Run 所选
          </button>
          <button
            disabled={busy || selectedSteps.length === 0}
            onClick={() => void executePipeline(false)}
          >
            运行所选
          </button>
        </div>
        <p className="meta">所选步骤会自动包含其依赖，执行顺序以 pipeline.toml 为准。</p>
        {pipelineSteps.length > 0 && (
          <div className="pipeline-steps">
            {pipelineSteps.map(step => (
              <label className="pipeline-step" key={step.id}>
                <input
                  type="checkbox"
                  checked={selectedSteps.includes(step.id)}
                  onChange={() => toggleStep(step.id)}
                />
                <span>
                  <strong>{step.group} / {step.id}</strong>
                  <small>{step.description}</small>
                  <small>依赖：{step.depends_on.length ? step.depends_on.join(", ") : "无"}</small>
                </span>
              </label>
            ))}
          </div>
        )}
        {pipelineResult && <p className="pipeline-result">{pipelineResult}</p>}
        {pipelineLog && <pre className="pipeline-log">{pipelineLog}</pre>}
      </section>

      <section className="panel">
        <div className="panel-title">
          <h2>环境检查</h2>
          <button
            disabled={busy}
            onClick={() =>
              run(async () =>
                setDoctor(
                  await invoke<DoctorReport>("doctor_report", {config: null})
                )
              )
            }
          >
            运行检查
          </button>
        </div>
        {doctor && (
          <>
            <p className="meta">配置：{doctor.config}</p>
            <div className="checks">
              {doctor.checks.map((check) => (
                <div className={`check ${check.status}`} key={check.name}>
                  <strong>{check.name}</strong>
                  <span>{check.message}</span>
                </div>
              ))}
            </div>
          </>
        )}
      </section>

      <section className="panel">
        <h2>Profile</h2>
        <div className="row">
          <label>
            运营
            <select
              value={detectOperator}
              onChange={(event) => setDetectOperator(event.target.value)}
            >
              <option value="cygames">cygames</option>
              <option value="netease">netease</option>
            </select>
          </label>
          <label className="grow">
            游戏目录
            <input
              value={gameRoot}
              onChange={(event) => setGameRoot(event.target.value)}
              placeholder={
                detectOperator === "cygames"
                  ? "留空使用 Cygames 默认目录"
                  : "留空自动查找 Sevo 安装"
              }
            />
          </label>
        </div>
        {detectOperator === "cygames" && (
          <div className="row">
            <label className="grow">
              dotnet
              <input value={dotnetPath} onChange={(event) => setDotnetPath(event.target.value)} />
            </label>
            <label className="grow">
              Il2CppDumper 目录
              <input
                value={il2cppDumperDir}
                onChange={(event) => setIl2cppDumperDir(event.target.value)}
                placeholder="留空使用默认目录"
              />
            </label>
          </div>
        )}
        <div className="row">
          <button
            disabled={busy}
            onClick={() =>
              run(async () => {
                const result = await invoke<DetectionResult>("detect_profile", {
                  operator: detectOperator,
                  gameRoot: gameRoot || null,
                  dataRoot: null,
                  variant: detectOperator === "cygames" ? "Cht" : "Chs",
                  pid: null,
                  dotnet: dotnetPath,
                  il2cppDumperDir: il2cppDumperDir || null,
                  apiBase,
                  tokenFile:
                    detectOperator === "cygames" ? tokenPath || null : null,
                  output: "runtime-profile.detected.json"
                });
                setDetected(result);
                setProfilePath(result.profile_path);
              })
            }
          >
            {detectOperator === "cygames"
              ? "检测运行中客户端"
              : "生成国服 RuntimeProfile"}
          </button>
        </div>
        {detected && (
          <div className="result ok">
            <span>RuntimeProfile 已生成</span>
            <code>{detected.profile_id}</code>
            <code>{detected.profile_path}</code>
          </div>
        )}
        <label>
          文件
          <input
            value={profilePath}
            onChange={(event) => setProfilePath(event.target.value)}
          />
        </label>
        <button
          disabled={busy}
          onClick={() =>
            run(async () =>
              setProfile(
                await invoke<ValidationReport>("validate_profile", {
                  path: profilePath
                })
              )
            )
          }
        >
          校验
        </button>
        {profile && (
          <div className={profile.ok ? "result ok" : "result bad"}>
            <strong>{profile.ok ? "通过" : "失败"}</strong>
            <span>{profile.operator ?? ""}</span>
            {profile.issues.map((issue) => (
              <code key={issue}>{issue}</code>
            ))}
          </div>
        )}
      </section>

      <section className="panel">
        <h2>账号材料</h2>
        <div className="row">
          <label>
            运营
            <select
              value={operator}
              onChange={(event) => setOperator(event.target.value)}
            >
              <option value="cygames">cygames</option>
              <option value="netease">netease</option>
            </select>
          </label>
          <label className="grow">
            路径
            <input
              value={credentialPath}
              onChange={(event) => setCredentialPath(event.target.value)}
              placeholder="目录或 svwb-cn-auth 文件"
            />
          </label>
        </div>
        <button
          disabled={busy || !credentialPath}
          onClick={() =>
            run(async () =>
              setCredential(
                await invoke<CredentialSummary>("credential_inspect", {
                  operator,
                  path: credentialPath
                })
              )
            )
          }
        >
          检查
        </button>
        {credential && (
          <div className={credential.valid ? "result ok" : "result bad"}>
            <strong>{credential.valid ? "材料有效" : "材料无效"}</strong>
            <span>{credential.kind}</span>
            <code>{credential.fingerprint}</code>
            {credential.issues.map((issue) => (
              <code key={issue}>{issue}</code>
            ))}
          </div>
        )}
      </section>

      <section className="panel">
        <h2>Runtime Probe</h2>
        <div className="row">
          <label className="grow">
            WBArts API
            <input value={apiBase} onChange={(event) => setApiBase(event.target.value)} />
          </label>
          <label className="grow">
            JWT 文件
            <input
              value={tokenPath}
              onChange={(event) => setTokenPath(event.target.value)}
              placeholder="0600 token file"
            />
          </label>
        </div>
        <button
          disabled={busy || !tokenPath}
          onClick={() =>
            run(async () =>
              setProbe(
                await invoke<RuntimeProbe>("probe_profile", {
                  path: profilePath,
                  apiBase,
                  tokenFile: tokenPath
                })
              )
            )
          }
        >
          执行 Probe
        </button>
        {probe && (
          <div className={probe.status === "success" ? "result ok" : "result bad"}>
            <strong>{probe.status}</strong>
            <span>{probe.request_scope}</span>
            {probe.resource_version && <code>resource_version: {probe.resource_version}</code>}
            {probe.app_version && <code>app_version: {probe.app_version}</code>}
            {probe.error && <code>{probe.error}</code>}
          </div>
        )}
        <div className="row">
          <button
            disabled={busy || !tokenPath}
            onClick={() =>
              run(async () =>
                setPublished(
                  await invoke<PublishResult>("publish_profile", {
                    path: profilePath,
                    apiBase,
                    tokenFile: tokenPath
                  })
                )
              )
            }
          >
            发布候选
          </button>
          <button
            disabled={busy || !tokenPath || !published}
            onClick={() =>
              run(async () => {
                if (published) {
                  await invoke("activate_profile", {
                    profileId: published.profile_id,
                    apiBase,
                    tokenFile: tokenPath
                  });
                }
              })
            }
          >
            激活候选
          </button>
        </div>
        {published && (
          <div className="result ok">
            <span>{published.created ? "已创建" : "已存在"}</span>
            <code>{published.profile_id}</code>
          </div>
        )}
      </section>
    </main>
  );
}
