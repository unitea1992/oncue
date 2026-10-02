import { useEffect, useId, useState } from 'react';
import { Activity, AlertTriangle, ExternalLink, MailPlus, Pencil, Plus, RefreshCw, Search, Trash2 } from 'lucide-react';
import { api, formatInvokeError, selectionStatusLabel, type Preset } from '../lib/tauri';
import { isExpiredOnce } from '../lib/schedule';
import { formatHm, formatMonthDay, nextOccurrence, weekdayName } from '../lib/format';
import { PresetForm } from '../components/PresetForm';
import { ConfirmDialog } from '../components/ConfirmDialog';

interface PresetTestResult {
  tone: 'info' | 'success' | 'warning';
  message: string;
}

type DialogState =
  | { kind: 'delete'; presetId: string; title: string; message: string }
  | { kind: 'action'; presetId: string; action: 'launch' | 'selfInvite'; title: string; message: string };

/** 一覧の左に大きく出す時刻と、その下の小さな説明 */
function whenOf(preset: Preset): { time: string; sub: string } {
  const s = preset.schedule;
  if (s.kind === 'once') {
    const date = new Date(s.startsAt);
    if (Number.isNaN(date.getTime())) return { time: '--:--', sub: '日時未定' };
    return { time: formatHm(date), sub: `${formatMonthDay(date)} 単発` };
  }
  return { time: s.time.slice(0, 5), sub: `${s.kind === 'weekly' ? '毎週' : '隔週'} ${weekdayName(s.weekday)}` };
}

interface EventItemProps {
  preset: Preset;
  selected: boolean;
  debugMode: boolean;
  runningTestKey: string | null;
  testResult?: PresetTestResult;
  onToggleSelected: () => void;
  onEdit: () => void;
  onDelete: () => void;
  onTestGroup: () => void;
  onTestInstances: () => void;
  onAction: (action: 'launch' | 'selfInvite') => void;
}

function EventItem({
  preset,
  selected,
  debugMode,
  runningTestKey,
  testResult,
  onToggleSelected,
  onEdit,
  onDelete,
  onTestGroup,
  onTestInstances,
  onAction,
}: EventItemProps) {
  const nameId = useId();
  const when = whenOf(preset);
  const expired = preset.schedule.kind === 'once' && isExpiredOnce(preset.schedule.startsAt);
  const next = preset.schedule.kind !== 'once' ? nextOccurrence(preset) : null;
  const facts = [
    preset.groupName || 'グループ名は未確認',
    preset.preferredInstanceName ? `会場「${preset.preferredInstanceName}」を優先` : null,
    next ? `次回 ${formatMonthDay(next)}` : null,
    preset.schedule.kind === 'once' && preset.sourceEventId ? 'カレンダーから登録' : null,
  ].filter(Boolean);

  return (
    <li className={`event-item${selected ? ' is-selected' : ''}${expired ? ' is-expired' : ''}`}>
      <div className="event-when">
        <span className="event-when-time num">{when.time}</span>
        <span className="event-when-sub">{when.sub}</span>
      </div>
      <div className="event-main">
        <span className="event-name" id={nameId}>
          {preset.label}
          {expired && <span className="tag tag-full">終了</span>}
        </span>
        <span className="event-facts">
          {facts.map((fact) => (
            <span key={fact}>{fact}</span>
          ))}
        </span>
      </div>
      <div className="event-pick">
        {selected ? (
          <>
            <span className="event-picked">
              <span className="event-picked-dot" aria-hidden />
              監視するイベント
            </span>
            <button type="button" className="quiet" onClick={onToggleSelected} aria-describedby={nameId}>
              外す
            </button>
          </>
        ) : (
          <button type="button" onClick={onToggleSelected} aria-describedby={nameId}>
            これを監視する
          </button>
        )}
      </div>
      <div className="event-tools">
        <button type="button" className="quiet icon" onClick={onEdit} aria-label={`${preset.label}を編集`}>
          <Pencil size={16} aria-hidden />
        </button>
        <button type="button" className="quiet icon danger-text" onClick={onDelete} aria-label={`${preset.label}を削除`}>
          <Trash2 size={16} aria-hidden />
        </button>
      </div>

      {debugMode && (
        <details className="event-debug">
          <summary>動作確認</summary>
          <div className="event-debug-body">
            <div className="event-debug-buttons">
              <button type="button" onClick={onTestGroup} disabled={!!runningTestKey}>
                <Search size={14} aria-hidden />
                {runningTestKey === `${preset.id}:group` ? '確認しています…' : 'グループ情報'}
              </button>
              <button type="button" onClick={onTestInstances} disabled={!!runningTestKey}>
                <Activity size={14} aria-hidden />
                {runningTestKey === `${preset.id}:instances` ? '確認しています…' : '会場の確認'}
              </button>
              <button type="button" onClick={() => onAction('launch')} disabled={!!runningTestKey}>
                <ExternalLink size={14} aria-hidden />
                {runningTestKey === `${preset.id}:launch` ? '開いています…' : '起動リンク'}
              </button>
              <button type="button" onClick={() => onAction('selfInvite')} disabled={!!runningTestKey}>
                <MailPlus size={14} aria-hidden />
                {runningTestKey === `${preset.id}:selfInvite` ? '送っています…' : 'セルフ招待'}
              </button>
            </div>
            {testResult && (
              <div
                className={`callout ${testResult.tone === 'warning' ? 'warning' : testResult.tone === 'success' ? 'success' : ''}`}
                role="status"
              >
                <p className="callout-body event-debug-result">{testResult.message}</p>
              </div>
            )}
          </div>
        </details>
      )}
    </li>
  );
}

export function PresetsPage() {
  const [presets, setPresets] = useState<Preset[]>([]);
  const [selectedPresetId, setSelectedPresetId] = useState<string | null>(null);
  const [showForm, setShowForm] = useState(false);
  const [editingPreset, setEditingPreset] = useState<Preset | null>(null);
  const [debugMode, setDebugMode] = useState(false);
  const [loading, setLoading] = useState(true);
  const [runningTestKey, setRunningTestKey] = useState<string | null>(null);
  const [testResults, setTestResults] = useState<Record<string, PresetTestResult>>({});
  const [dialogState, setDialogState] = useState<DialogState | null>(null);
  const [pageError, setPageError] = useState<string | null>(null);

  useEffect(() => {
    loadPresets();
    loadConfig();
  }, []);

  const loadPresets = async () => {
    try {
      setPresets(await api.presets.getPresets());
    } catch (err) {
      setPageError(formatInvokeError(err, 'イベントの一覧を読み込めませんでした。\nもう一度試してください。'));
    } finally {
      setLoading(false);
    }
  };

  const loadConfig = async () => {
    try {
      const config = await api.config.getConfig();
      setSelectedPresetId(config.selectedPresetId ?? null);
      setDebugMode(!!config.debugMode);
    } catch (err) {
      setPageError(formatInvokeError(err, '監視するイベントを読み込めませんでした。\nもう一度試してください。'));
    }
  };

  const handleRetry = () => {
    setPageError(null);
    void loadPresets();
    void loadConfig();
  };

  const openForm = (preset: Preset | null) => {
    setEditingPreset(preset);
    setShowForm(true);
  };

  const handleToggleSelected = async (presetId: string, selected: boolean) => {
    try {
      await api.config.saveConfigPatch({ selectedPresetId: selected ? null : presetId });
      setSelectedPresetId(selected ? null : presetId);
    } catch (err) {
      setPageError(formatInvokeError(err, '監視するイベントを保存できませんでした。\nもう一度試してください。'));
    }
  };

  const handleSave = async (presetData: Omit<Preset, 'id'>): Promise<boolean> => {
    try {
      const preset: Preset = editingPreset
        ? { ...editingPreset, ...presetData }
        : { ...presetData, id: `preset-${Date.now()}` };
      await api.presets.savePreset(preset);
      await loadPresets();
      setShowForm(false);
      setEditingPreset(null);
      return true;
    } catch {
      // falseでフォームを開いたままにし、入力保持はフォーム側で行う
      return false;
    }
  };

  const handleDelete = async (id: string) => {
    try {
      await api.presets.deletePreset(id);
    } catch (err) {
      setPageError(formatInvokeError(err, '削除できませんでした。\nもう一度試してください。'));
      return;
    }
    // 削除したイベントが選択中なら、保存側の選択も外す（残すと存在しないイベントを指したままになる）
    if (id === selectedPresetId) {
      setSelectedPresetId(null);
      try {
        await api.config.saveConfigPatch({ selectedPresetId: null });
      } catch {
        /* 監視画面は存在しない選択を「未選択」として扱うため、表示には影響しない */
      }
    }
    await loadPresets();
  };

  const setTestResult = (presetId: string, result: PresetTestResult) => {
    setTestResults((prev) => ({ ...prev, [presetId]: result }));
  };

  const handleTestGroupInfo = async (presetId: string) => {
    setRunningTestKey(`${presetId}:group`);
    try {
      const result = await api.monitor.testGroupInfo(presetId);
      setTestResult(presetId, {
        tone: 'success',
        message: `グループ: ${result.groupName}${result.shortCode ? ` (${result.shortCode})` : ''}`,
      });
    } catch (err) {
      setTestResult(presetId, { tone: 'warning', message: formatInvokeError(err, 'グループ情報を確認できませんでした。') });
    } finally {
      setRunningTestKey(null);
    }
  };

  const handleTestInstances = async (presetId: string) => {
    setRunningTestKey(`${presetId}:instances`);
    try {
      const result = await api.monitor.testInstanceCheck(presetId);
      setTestResult(presetId, {
        tone: result.selectionStatus === 'ready' ? 'success' : result.selectionStatus === 'waiting' ? 'info' : 'warning',
        message: `${selectionStatusLabel(result.selectionStatus)}: ${result.message}`,
      });
    } catch (err) {
      setTestResult(presetId, { tone: 'warning', message: formatInvokeError(err, '会場を確認できませんでした。') });
    } finally {
      setRunningTestKey(null);
    }
  };

  const handleRunApiAction = async (presetId: string, action: 'launch' | 'selfInvite') => {
    setRunningTestKey(`${presetId}:${action}`);
    try {
      const result = await api.monitor.runPresetApiAction(presetId, action);
      setTestResult(presetId, { tone: 'info', message: result.message });
    } catch (err) {
      setTestResult(presetId, { tone: 'warning', message: formatInvokeError(err, '実行できませんでした。') });
    } finally {
      setRunningTestKey(null);
    }
  };

  const selected = presets.find((p) => p.id === selectedPresetId) ?? null;

  return (
    <div className="page events">
      <div className="page-head">
        <h1>イベント</h1>
        {!loading && presets.length > 0 && (
          <button type="button" className="primary" onClick={() => openForm(null)}>
            <Plus size={16} aria-hidden />
            イベントを追加
          </button>
        )}
      </div>

      {pageError && (
        <div className="callout error" role="alert">
          <AlertTriangle size={16} aria-hidden />
          <div className="callout-body">
            <p className="error-message">{pageError}</p>
            <button type="button" onClick={handleRetry}>
              <RefreshCw size={14} aria-hidden />
              読み込み直す
            </button>
          </div>
        </div>
      )}

      {loading ? (
        <p className="page-status" role="status">
          イベントを読み込んでいます…
        </p>
      ) : presets.length === 0 ? (
        <section className="events-empty" data-testid="events-empty" aria-labelledby="events-empty-title">
          <h2 id="events-empty-title">まだイベントがありません</h2>
          <p>
            入りたいグループイベントを登録すると、
            <br />
            「監視」画面から開始の瞬間を待ち受けられます。
          </p>
          <button type="button" className="primary large" onClick={() => openForm(null)}>
            <Plus size={16} aria-hidden />
            イベントを追加
          </button>
        </section>
      ) : (
        <>
          <ul className="event-list" aria-label="登録したイベント">
            {presets.map((preset) => (
              <EventItem
                key={preset.id}
                preset={preset}
                selected={selectedPresetId === preset.id}
                debugMode={debugMode}
                runningTestKey={runningTestKey}
                testResult={testResults[preset.id]}
                onToggleSelected={() => handleToggleSelected(preset.id, selectedPresetId === preset.id)}
                onEdit={() => openForm(preset)}
                onDelete={() =>
                  setDialogState({
                    kind: 'delete',
                    presetId: preset.id,
                    title: 'イベントを削除',
                    message: `「${preset.label}」を削除します。\nこの操作は取り消せません。`,
                  })
                }
                onTestGroup={() => handleTestGroupInfo(preset.id)}
                onTestInstances={() => handleTestInstances(preset.id)}
                onAction={(action) =>
                  setDialogState(
                    action === 'launch'
                      ? {
                          kind: 'action',
                          presetId: preset.id,
                          action,
                          title: '起動リンクを開く',
                          message: '対象の会場を VRChat で開きます。\nVRChat 側の動きを確かめるときに使います。',
                        }
                      : {
                          kind: 'action',
                          presetId: preset.id,
                          action,
                          title: 'セルフ招待を送る',
                          message: '対象の会場への招待を、自分宛てに送ります。\nVRChat の通知を確かめてください。',
                        },
                  )
                }
              />
            ))}
          </ul>
          <p className="hint">
            監視できるのは一度に1件です。
            <br />
            {selected
              ? '選んだイベントは、アプリを開いていれば時間になると自動で監視します。'
              : '「これを監視する」で選んでください。'}
          </p>
        </>
      )}

      {showForm && (
        <PresetForm
          preset={editingPreset}
          onSave={handleSave}
          onCancel={() => {
            setShowForm(false);
            setEditingPreset(null);
          }}
        />
      )}

      <ConfirmDialog
        open={!!dialogState}
        title={dialogState?.title || ''}
        message={dialogState?.message || ''}
        confirmLabel={dialogState?.kind === 'delete' ? '削除する' : '実行する'}
        tone={dialogState?.kind === 'delete' ? 'danger' : 'default'}
        busy={!!runningTestKey}
        onCancel={() => setDialogState(null)}
        onConfirm={async () => {
          if (!dialogState) return;
          const currentDialog = dialogState;
          setDialogState(null);
          if (currentDialog.kind === 'delete') {
            await handleDelete(currentDialog.presetId);
            return;
          }
          await handleRunApiAction(currentDialog.presetId, currentDialog.action);
        }}
      />
    </div>
  );
}
