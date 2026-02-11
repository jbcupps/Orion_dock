/**
 * Birth stage display messages — aligned with orion-birth (Darkness → Emergence).
 * Same UX as Orion/Abigail: darkness, birth flow, then operation.
 */
export const BIRTH_STAGE_MESSAGES: Record<string, string> = {
  Darkness: 'Generating identity...',
  Ignition: 'Configure your local LLM.',
  Connectivity: 'Establishing connections...',
  Genesis: 'Discovering who I am...',
  Emergence: 'I am awake.',
};

export const OPERATION_MESSAGE = 'I am awake.';

export function stageDisplayMessage(stage: string | null): string {
  if (!stage) return BIRTH_STAGE_MESSAGES['Darkness'] ?? 'Generating identity...';
  return BIRTH_STAGE_MESSAGES[stage] ?? stage;
}
