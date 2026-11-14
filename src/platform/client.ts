import { invoke, isTauri } from '@tauri-apps/api/core';
import type {
  AppError,
  BootstrapSnapshot,
  Language,
  ErrorCode,
  SettingsSnapshot,
  UpdateSettings,
} from '../generated/contracts';

const codes: ReadonlySet<ErrorCode> = new Set([
  'storage',
  'unsupported_schema',
  'foreign_database',
  'conflict',
  'invalid_input',
  'busy',
  'unavailable',
  'limit_exceeded',
  'invalid_pattern',
  'not_found',
  'clipboard',
  'missing_file',
  'restricted',
  'target_lost',
  'shortcut_conflict',
  'query_timeout',
  'excluded',
  'duplicate_name',
]);

export function safeError(error: unknown): AppError {
  if (
    typeof error === 'object' &&
    error !== null &&
    'code' in error &&
    codes.has(error.code as ErrorCode)
  ) {
    // Never display unknown renderer/plugin errors or stringify arbitrary objects.
    const messages: Record<ErrorCode, string> = {
      storage: 'could_not_save_locally_check_disk_space_and_permissions',
      unsupported_schema: 'open_this_data_with_a_newer_version',
      foreign_database: 'this_database_does_not_belong_to_uclip',
      conflict: 'data_has_changed_refresh_and_try_again',
      invalid_input: 'invalid_request_parameters',
      busy: 'the_local_service_is_busy_try_again_later',
      limit_exceeded: 'content_or_history_exceeds_the_limit_nothing_was_truncated_to_fit',
      invalid_pattern: 'invalid_expression_backreferences_and_lookaround_are_not_supported',
      not_found: 'this_record_no_longer_exists_refresh_the_list',
      clipboard: 'the_system_clipboard_is_unavailable_try_again',
      missing_file: 'some_files_or_folders_no_longer_exist_the_clipboard_was_not_changed',
      restricted: 'this_platform_or_its_permissions_do_not_support_this_action',
      target_lost: 'the_original_app_target_changed_automatic_paste_was_not_performed',
      shortcut_conflict:
        'the_shortcut_is_duplicated_occupied_or_reserved_by_the_system_previous_settings_were_kept',
      excluded: 'content_matched_an_exclusion_rule_and_was_not_saved',
      duplicate_name: 'a_collection_with_this_name_already_exists_choose_another_name',
      query_timeout: 'search_timed_out_narrow_the_scope_or_simplify_the_expression',
      unavailable: 'the_local_service_is_unavailable_restart_the_desktop_app',
    };
    const code = error.code as ErrorCode;
    return { code, message: messages[code] };
  }
  return {
    code: 'unavailable',
    message: 'cannot_connect_to_the_desktop_service_start_the_app_through_tauri',
  };
}

async function call<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  if (!isTauri()) throw safeError(null);
  try {
    return await invoke<T>(command, args);
  } catch (error) {
    throw safeError(error);
  }
}

export const desktop = {
  async bootstrap(): Promise<BootstrapSnapshot> {
    const snapshot = await call<BootstrapSnapshot>('bootstrap');
    if (snapshot.contractVersion !== 2) {
      throw {
        code: 'unavailable',
        message: 'the_interface_and_desktop_service_versions_do_not_match_restart_or_reinstall',
      } satisfies AppError;
    }
    return snapshot;
  },
  setUiLanguage: (language: Exclude<Language, 'system'>) =>
    call<void>('set_ui_language', { language }),
  settings: () => call<SettingsSnapshot>('get_settings'),
  updateSettings: (request: UpdateSettings) =>
    call<SettingsSnapshot>('update_settings', { request }),
};
