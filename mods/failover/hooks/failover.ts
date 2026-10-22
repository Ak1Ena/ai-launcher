import type { Register } from 'claude-code'

// Acts only when ai-launcher's failover started this claude: it sets AI_FO_HANDOFF and
// relaunches the session on the next account once this process ends.
export const register: Register = on => {
  // While failover runs this claude, the status line names this account and the next one.
  on('session.start', async ($, e, next) => {
    const result = await next(e)
    const member = await $.env.get('AI_FO_MEMBER')
    if (member) {
      const after = await $.env.get('AI_FO_NEXT')
      $.ui.status(`⇄ ${member} · ${after ? `next: ${after}` : 'last account'}`)
    }
    return result
  })

  on('classic.StopFailure', async ($, e, next) => {
    const result = await next(e)
    const handoff = await $.env.get('AI_FO_HANDOFF')
    if (e.error !== 'rate_limit' || e.agent_id || !handoff) return result

    const member = (await $.env.get('AI_FO_MEMBER')) ?? 'this account'
    await $.fs.write(handoff, JSON.stringify({
      session_id: e.session_id,
      transcript_path: e.transcript_path,
      member,
      error_details: e.error_details ?? null,
    }))
    $.ui.toast(`${member} hit its limit, switching account`)
    // The shell's parent is this claude.
    const kill = await $.process.run(['sh', '-c', 'kill -TERM "$PPID"'])
    if (kill.exitCode !== 0) $.ui.toast(`could not end claude: ${kill.stderr.trim()}`)
    return result
  })
}
