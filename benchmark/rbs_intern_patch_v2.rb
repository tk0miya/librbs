# frozen_string_literal: true

# Variant 2: targeted intern in the resolver only.
#
# The v1 patch (override-`.new`) traded object allocation for Hash#[]
# overhead on every TypeName / Namespace construction, including the
# parsing phase where most names are unique. The benchmark showed it was
# essentially break-even.
#
# This v2 attacks the resolver alone:
#
#   - Adds `RBS::TypeName.intern(namespace:, name:)` and
#     `RBS::Namespace.intern(path:, absolute:)` that hash-cons.
#   - Memoizes `#hash` on both classes (the resolver cache key
#     `[type_name, context]` recomputes both per probe).
#   - Memoizes `TypeName#to_namespace`.
#   - Monkey-patches `RBS::Resolver::TypeNameResolver` to call `.intern`
#     instead of `.new` in `resolve_namespace0`, `resolve_type_name`,
#     and `resolve_head_namespace`.
#
# Parsing is left untouched.

require "rbs"

module RBS
  class Namespace
    NS_INTERN = {}

    def self.intern(path:, absolute:)
      absolute = absolute ? true : false
      NS_INTERN[[path, absolute]] ||= begin
        frozen_path = path.frozen? ? path : path.dup.freeze
        new(path: frozen_path, absolute: absolute)
      end
    end

    alias_method :__librbs_orig_hash, :hash
    def hash
      @__librbs_hash ||= __librbs_orig_hash
    end
  end

  class TypeName
    TN_INTERN = {}

    def self.intern(namespace:, name:)
      TN_INTERN[[namespace, name]] ||= new(namespace: namespace, name: name)
    end

    alias_method :__librbs_orig_hash, :hash
    def hash
      @__librbs_hash ||= __librbs_orig_hash
    end

    alias_method :__librbs_orig_to_namespace, :to_namespace
    def to_namespace
      @__librbs_to_namespace ||= __librbs_orig_to_namespace
    end
  end

  module Resolver
    class TypeNameResolver
      # Replace `TypeName.new` with `TypeName.intern` in the three hot
      # paths so the inject loop and the recursive walk reuse a shared
      # instance per `(namespace, name)` pair, which then carries a
      # memoized hash and to_namespace.

      def resolve(type_name, context:)
        if type_name.absolute? && has_type_name?(type_name)
          return type_name
        end

        try_cache([type_name, context]) do
          if type_name.class?
            resolve_namespace0(type_name, context, Set.new) || nil
          else
            namespace = type_name.namespace

            if namespace.empty?
              resolve_type_name(type_name.name, context)
            else
              if namespace = resolve_namespace0(namespace.to_type_name, context, Set.new)
                type_name = TypeName.intern(name: type_name.name, namespace: namespace.to_namespace)
                has_type_name?(type_name)
              end
            end
          end
        end
      end

      def resolve_type_name(type_name, context)
        if context
          outer, inner = context
          case inner
          when false
            resolve_type_name(type_name, outer)
          else
            has_type_name?(inner) or raise "Context must be normalized: #{inner.inspect}"
            has_type_name?(TypeName.intern(name: type_name, namespace: inner.to_namespace)) || resolve_type_name(type_name, outer)
          end
        else
          has_type_name?(TypeName.intern(name: type_name, namespace: Namespace.root))
        end
      end

      def resolve_head_namespace(head, context)
        if context
          outer, inner = context
          case inner
          when false
            resolve_head_namespace(head, outer)
          when TypeName
            has_type_name?(inner) or raise "Context must be normalized: #{inner.inspect}"
            type_name = TypeName.intern(name: head, namespace: inner.to_namespace)
            has_type_name?(type_name) || aliased_name?(type_name) || resolve_head_namespace(head, outer)
          end
        else
          type_name = TypeName.intern(name: head, namespace: Namespace.root)
          has_type_name?(type_name) || aliased_name?(type_name)
        end
      end

      def resolve_namespace0(type_name, context, visited)
        head, *tail = [*type_name.namespace.path, type_name.name]
        head = head #: Symbol

        head =
          if type_name.absolute?
            root_name = TypeName.intern(name: head, namespace: Namespace.root)
            has_type_name?(root_name) || aliased_name?(root_name)
          else
            resolve_head_namespace(head, context)
          end

        if head
          if (rhs, context = aliases.fetch(head, nil))
            head = normalize_namespace(head, rhs, context, visited) or return head
          end

          tail.inject(head) do |namespace, name|
            type_name = TypeName.intern(name: name, namespace: namespace.to_namespace)
            case
            when has_type_name?(type_name)
              type_name
            when (rhs, context = aliases.fetch(type_name, nil))
              m = normalize_namespace(type_name, rhs, context, visited) or return m
            else
              return nil
            end
          end
        end
      end
    end
  end
end
