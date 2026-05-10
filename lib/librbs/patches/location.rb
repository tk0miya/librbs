# frozen_string_literal: true

module Librbs
  module Patches
    # Defer `RBS::Location#add_required_child` / `add_optional_child`
    # until any reader (`[]`, `each_required_key`, `_required_keys`, …)
    # touches them. The native materialiser pushes child specs onto
    # `@__librbs_pending_children` instead of calling C-side
    # `_add_required_child` per child — for the load + resolve +
    # add_source path, no reader runs, so the children never get built.
    #
    # The pending array's shape mirrors the three native callbacks:
    #   [:required,         <name Symbol>, <Integer start>, <Integer end>]
    #   [:optional_present, <name Symbol>, <Integer start>, <Integer end>]
    #   [:optional_absent,  <name Symbol>]
    module Location
      def __librbs_realise_children
        pending = @__librbs_pending_children
        return unless pending
        @__librbs_pending_children = nil
        pending.each do |row|
          case row[0]
          when :required
            _add_required_child(row[1], row[2], row[3])
          when :optional_present
            _add_optional_child(row[1], row[2], row[3])
          when :optional_absent
            _add_optional_no_child(row[1])
          end
        end
      end

      # Reader-side override. Each method first realises pending
      # children, then defers to the upstream implementation.
      %i[
        []
        each_required_key
        each_optional_key
        _required_keys
        _optional_keys
        key?
        optional_key?
        required_key?
        local_location
        local_source
        inspect
      ].each do |m|
        define_method(m) do |*args, &block|
          __librbs_realise_children
          super(*args, &block)
        end
      end
    end
  end
end

RBS::Location.prepend(Librbs::Patches::Location)
