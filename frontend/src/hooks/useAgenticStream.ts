import { useCallback, useEffect, useRef, useState } from 'react';
import {
  startAgenticRun,
  subscribeToAgenticStream,
  respondToAgent,
  confirmAgentTool,
  cancelAgenticRun,
} from '../api';

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

export interface AgenticEntry {
  id: number;
  type: string;
  turn: number;
  content: string;
  toolName?: string;
  success?: boolean;
  timestamp: number;
}

export type RunStatus =
  | 'idle'
  | 'running'
  | 'waiting_for_mentor'
  | 'waiting_for_confirmation'
  | 'completed'
  | 'failed'
  | 'cancelled';

export interface StartOptions {
  maxTurns: number;
  autoApprove: boolean;
  routerMode: 'auto' | 'think_hard' | 'think_harder';
}

// ---------------------------------------------------------------------------
// Hook
// ---------------------------------------------------------------------------

export function useAgenticStream(
  agentId: string,
  onError: (message: string) => void,
) {
  const [status, setStatus] = useState<RunStatus>('idle');
  const [taskId, setTaskId] = useState<string | null>(null);
  const [turn, setTurn] = useState(0);
  const [toolCallCount, setToolCallCount] = useState(0);
  const [entries, setEntries] = useState<AgenticEntry[]>([]);
  const [starting, setStarting] = useState(false);

  // Latest thinking content (useful for live-display)
  const [activeThought, setActiveThought] = useState<string | null>(null);
  // Summary from the done event
  const [doneSummary, setDoneSummary] = useState<string | null>(null);
  // Mentor question text
  const [mentorQuestion, setMentorQuestion] = useState<string | null>(null);
  // Confirmation tool info
  const [confirmationInfo, setConfirmationInfo] = useState<{
    toolName: string;
    args: string;
  } | null>(null);

  const eventSourceRef = useRef<{ close: () => void } | null>(null);
  const entryIdRef = useRef(0);

  // Cleanup on unmount
  useEffect(() => {
    return () => {
      eventSourceRef.current?.close();
    };
  }, []);

  const addEntry = useCallback(
    (type: string, turnNum: number, content: string, extra?: Partial<AgenticEntry>) => {
      const id = ++entryIdRef.current;
      setEntries((prev) => [
        ...prev,
        { id, type, turn: turnNum, content, timestamp: Date.now(), ...extra },
      ]);
    },
    [],
  );

  const attachToTask = useCallback(
    (tid: string) => {
      eventSourceRef.current?.close();
      const es = subscribeToAgenticStream(
        agentId,
        tid,
        (eventName, rawData) => {
          const envelope = rawData as Record<string, unknown>;
          const data = (envelope.data ?? envelope) as Record<string, unknown>;

          switch (eventName) {
            case 'thinking':
              setTurn(data.turn as number);
              setActiveThought(data.content as string);
              addEntry('thinking', data.turn as number, data.content as string);
              break;
            case 'tool_call':
              setActiveThought(`Calling ${data.tool_name}...`);
              setToolCallCount((c) => c + 1);
              addEntry(
                'tool_call',
                data.turn as number,
                JSON.stringify(data.arguments, null, 2),
                { toolName: data.tool_name as string },
              );
              break;
            case 'tool_result':
              addEntry('tool_result', data.turn as number, data.output as string, {
                toolName: data.tool_name as string,
                success: data.success as boolean,
              });
              break;
            case 'mentor_needed':
              setStatus('waiting_for_mentor');
              setTurn(data.turn as number);
              setMentorQuestion(data.question as string);
              setActiveThought(null);
              addEntry('mentor_needed', data.turn as number, data.question as string);
              break;
            case 'confirmation_needed':
              setStatus('waiting_for_confirmation');
              setTurn(data.turn as number);
              setConfirmationInfo({
                toolName: data.tool_name as string,
                args: JSON.stringify(data.arguments, null, 2),
              });
              setActiveThought(null);
              addEntry(
                'confirmation_needed',
                data.turn as number,
                `Tool: ${data.tool_name}\n${JSON.stringify(data.arguments, null, 2)}`,
                { toolName: data.tool_name as string },
              );
              break;
            case 'done':
              setStatus((data.status as string) === 'cancelled' ? 'cancelled' : 'completed');
              setDoneSummary(data.summary as string);
              setActiveThought(null);
              addEntry('done', data.turns_used as number, data.summary as string);
              es.close();
              break;
            case 'error':
              setStatus('failed');
              setActiveThought(null);
              addEntry('error', 0, data.message as string);
              es.close();
              break;
          }
        },
        () => {
          // SSE connection error — could be normal close or network issue
        },
      );
      eventSourceRef.current = es;
    },
    [addEntry, agentId],
  );

  const start = useCallback(
    async (goal: string, opts: StartOptions) => {
      if (!goal.trim() || starting) return;

      setStarting(true);
      setEntries([]);
      setTurn(0);
      setToolCallCount(0);
      entryIdRef.current = 0;
      setActiveThought(null);
      setDoneSummary(null);
      setMentorQuestion(null);
      setConfirmationInfo(null);

      try {
        const result = await startAgenticRun(agentId, {
          goal: goal.trim(),
          max_turns: opts.maxTurns,
          auto_approve_safe_tools: opts.autoApprove,
          router_mode: opts.routerMode,
        });
        setTaskId(result.task_id);
        setStatus('running');
        attachToTask(result.task_id);
      } catch (e) {
        onError(e instanceof Error ? e.message : 'Failed to start agentic run');
        setStatus('idle');
      } finally {
        setStarting(false);
      }
    },
    [agentId, starting, onError, attachToTask],
  );

  const respondToMentor = useCallback(
    async (response: string) => {
      if (!taskId || !response.trim()) return;
      try {
        await respondToAgent(agentId, taskId, response.trim());
        addEntry('mentor_response', turn, response.trim());
        setMentorQuestion(null);
        setStatus('running');
      } catch (e) {
        onError(e instanceof Error ? e.message : 'Failed to send response');
      }
    },
    [agentId, taskId, turn, onError, addEntry],
  );

  const confirm = useCallback(
    async (approved: boolean) => {
      if (!taskId) return;
      try {
        await confirmAgentTool(agentId, taskId, approved);
        addEntry('confirmation_response', turn, approved ? 'Approved' : 'Denied');
        setConfirmationInfo(null);
        setStatus('running');
      } catch (e) {
        onError(e instanceof Error ? e.message : 'Failed to confirm');
      }
    },
    [agentId, taskId, turn, onError, addEntry],
  );

  const cancel = useCallback(async () => {
    if (!taskId) return;
    try {
      await cancelAgenticRun(agentId, taskId);
    } catch (e) {
      onError(e instanceof Error ? e.message : 'Failed to cancel');
    }
  }, [agentId, taskId, onError]);

  const reset = useCallback(() => {
    eventSourceRef.current?.close();
    setStatus('idle');
    setTaskId(null);
    setEntries([]);
    setTurn(0);
    setToolCallCount(0);
    setActiveThought(null);
    setDoneSummary(null);
    setMentorQuestion(null);
    setConfirmationInfo(null);
    entryIdRef.current = 0;
  }, []);

  const isActive =
    status === 'running' ||
    status === 'waiting_for_mentor' ||
    status === 'waiting_for_confirmation';

  return {
    status,
    taskId,
    turn,
    toolCallCount,
    entries,
    starting,
    activeThought,
    doneSummary,
    mentorQuestion,
    confirmationInfo,
    isActive,
    start,
    attachToTask,
    respondToMentor,
    confirm,
    cancel,
    reset,
  };
}
