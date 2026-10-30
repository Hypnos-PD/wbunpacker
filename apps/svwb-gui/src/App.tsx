import {useState} from "react";
import {invoke} from "@tauri-apps/api/core";

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

export default function App() {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [doctor, setDoctor] = useState<DoctorReport | null>(null);
  const [profilePath, setProfilePath] = useState(
    "contracts/examples/runtime-profile.cygames.json"
  );
  const [profile, setProfile] = useState<ValidationReport | null>(null);
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
          <label className="grow">
            游戏目录
            <input
              value={gameRoot}
              onChange={(event) => setGameRoot(event.target.value)}
              placeholder="留空使用 dumper 默认值"
            />
          </label>
        </div>
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
        <div className="row">
          <button
            disabled={busy}
            onClick={() =>
              run(async () => {
                const result = await invoke<DetectionResult>("detect_profile", {
                  gameRoot: gameRoot || null,
                  dataRoot: null,
                  variant: "Cht",
                  pid: null,
                  dotnet: dotnetPath,
                  il2cppDumperDir: il2cppDumperDir || null,
                  apiBase,
                  tokenFile: tokenPath || null,
                  output: "runtime-profile.detected.json"
                });
                setDetected(result);
                setProfilePath(result.profile_path);
              })
            }
          >
            检测运行中客户端
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
