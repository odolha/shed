import type { AiSession } from '../../api/gen/api';
import { deleteBody, laterSessions } from './sessions';

const session = (id: string, parent: string | null, title = id): AiSession => ({
  id,
  title,
  modified: Math.floor(Date.now() / 1000) - 3600,
  branch: null,
  prompts: 3,
  parent,
  depth: parent ? 1 : 0,
  draft: false,
  drafts: 0,
});

describe('deleting a session', () => {
  const all = [
    session('a', null),
    session('b', 'a'),
    session('bb', 'b'),
    session('c', 'a'),
    session('z', null),
  ];

  it('takes the sessions going on from it along', () => {
    expect(
      laterSessions(all, 'a')
        .map((s) => s.id)
        .sort(),
    ).toEqual(['b', 'bb', 'c']);
    expect(laterSessions(all, 'b').map((s) => s.id)).toEqual(['bb']);
    expect(laterSessions(all, 'z')).toEqual([]);
  });

  it('says what goes, and where', () => {
    const body = deleteBody(session('a', null, 'Plan the shelves'), 3);
    expect(body).toContain('“Plan the shelves”');
    expect(body).toContain('3 prompts, and 3 later sessions that continue it.');
    expect(body).toContain("Shed's trash (~/.local/state/shed/ai-trash), kept 30 days");
    expect(deleteBody(session('z', null), 0)).not.toContain('later');
    expect(deleteBody(session('z', null), 1)).toContain('and 1 later session that continues it');
  });
});
