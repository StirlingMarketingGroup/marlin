import { describe, it, expect, beforeEach, vi } from 'vitest';
import { render, screen, fireEvent, waitFor } from '@testing-library/react';
import { invoke } from '@tauri-apps/api/core';
import { emit } from '@tauri-apps/api/event';
import SmbConnectWindow from '../SmbConnectWindow';
import { SMB_CONNECT_SUCCESS_EVENT } from '@/utils/events';

vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(),
  listen: vi.fn(async () => () => {}),
}));

vi.mock('@tauri-apps/api/window', () => ({
  getCurrentWindow: () => ({
    close: vi.fn().mockResolvedValue(undefined),
  }),
}));

const mockInvoke = vi.mocked(invoke);
const mockEmit = vi.mocked(emit);

describe('SmbConnectWindow', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mockInvoke.mockImplementation(async (cmd: string) => {
      if (cmd === 'add_smb_server') {
        return { hostname: 'server', username: 'user', domain: null };
      }
      return undefined;
    });
  });

  async function submit(server: string) {
    render(<SmbConnectWindow />);
    fireEvent.change(screen.getByLabelText('Server'), { target: { value: server } });
    fireEvent.change(screen.getByLabelText('Username'), { target: { value: 'user' } });
    fireEvent.change(screen.getByLabelText('Password'), { target: { value: 'secret' } });
    fireEvent.click(screen.getByRole('button', { name: 'Connect' }));
  }

  it('saves a pasted address as the server name and keeps the share path', async () => {
    await submit('\\\\server\\share\\folder');

    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith(
        'add_smb_server',
        expect.objectContaining({ hostname: 'server' })
      );
    });
    expect(mockEmit).toHaveBeenCalledWith(
      SMB_CONNECT_SUCCESS_EVENT,
      expect.objectContaining({ targetPath: 'smb://server/share/folder' })
    );
  });

  it('keeps a user name and password out of the saved server name', async () => {
    await submit('smb://user:secret@server/share');

    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith(
        'add_smb_server',
        expect.objectContaining({ hostname: 'server' })
      );
    });
  });

  it('opens a bare server name at the share root', async () => {
    await submit('server');

    await waitFor(() => {
      expect(mockEmit).toHaveBeenCalledWith(
        SMB_CONNECT_SUCCESS_EVENT,
        expect.objectContaining({ targetPath: 'smb://server/' })
      );
    });
  });
});
