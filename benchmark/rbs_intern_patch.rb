# frozen_string_literal: true

# Ad-hoc proof-of-concept: hash-cons RBS::TypeName / RBS::Namespace at
# construction time and memoize TypeName#to_namespace.
#
# Target: the per-candidate allocations the type-name resolver pays inside
# `resolve_namespace0`, `resolve_type_name`, and `resolve_head_namespace`,
# each of which builds a fresh `TypeName.new(name:, namespace: x.to_namespace)`
# on every recursion / inject step. Hash-consing collapses those repeated
# constructions to a single shared instance and lets `Hash#[]` on
# `[type_name, context]` keys hit a precomputed identity hash.
#
# This patch is intentionally minimal — it does NOT touch the resolver
# source. Any speedup comes purely from sharing TypeName / Namespace
# identities across the codebase.
#
# Load order: this file must be required AFTER `rbs` so the originals exist.

require "rbs"

module RBS
  class Namespace
    NS_INTERN = {}

    class << self
      alias_method :__librbs_orig_new, :new

      def new(path:, absolute:)
        absolute = absolute ? true : false
        key = [path, absolute]
        if (hit = NS_INTERN[key])
          return hit
        end
        # Freeze the path so the cached key cannot be mutated out from
        # under us. Upstream callers do not mutate `path` after passing
        # it to `new`, so this is safe.
        frozen_path = path.frozen? ? path : path.dup.freeze
        stable_key = [frozen_path, absolute].freeze
        NS_INTERN[stable_key] ||= __librbs_orig_new(path: frozen_path, absolute: absolute)
      end
    end
  end

  class TypeName
    TN_INTERN = {}

    class << self
      alias_method :__librbs_orig_new, :new

      def new(namespace:, name:)
        key = [namespace, name]
        TN_INTERN[key] ||= __librbs_orig_new(namespace: namespace, name: name)
      end
    end

    # The resolver's inject loop calls `to_namespace` on the same
    # TypeName many times across different `resolve` calls. With
    # hash-consing the receiver is shared, so memoizing here turns
    # repeated `Namespace.new` allocations into a single ivar read.
    alias_method :__librbs_orig_to_namespace, :to_namespace
    def to_namespace
      @__librbs_to_namespace ||= __librbs_orig_to_namespace
    end
  end
end
