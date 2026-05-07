# frozen_string_literal: true

require "rbs"
require "librbs/version"

begin
  require "librbs/librbs"
  require "librbs/patches"
rescue LoadError => e
  warn "[librbs] native extension failed to load: #{e.message}"
  warn "[librbs] falling back to pure RBS implementation"
end
