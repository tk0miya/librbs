# frozen_string_literal: true

require "open3"
require "rbconfig"

# Run the given Ruby `code` in a fresh subprocess that does **not** load
# librbs. Returns the captured stdout as a String. Raises if the
# subprocess exits non-zero.
#
# Used by compat specs to obtain a baseline from pure RBS without our
# monkey-patches polluting the in-process environment.
def without_librbs(code)
  out, err, status = Open3.capture3(RbConfig.ruby, "-e", code)
  unless status.success?
    raise "without_librbs subprocess failed (#{status}):\nSTDERR:\n#{err}"
  end
  out
end
