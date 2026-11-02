"""Resolve Python source imports and paths without executing target modules."""
from __future__ import annotations

import ast
from pathlib import Path

AMBIGUOUS = object()


def path_value(node, bindings):
    if isinstance(node, ast.Constant) and isinstance(node.value, (str, int)):
        return node.value
    if isinstance(node, ast.Name):
        value = bindings.get(node.id)
        if value is AMBIGUOUS:
            raise ValueError('ambiguous imported path constant')
        return value
    if isinstance(node, ast.BinOp) and isinstance(node.op, ast.Div):
        left, right = path_value(node.left, bindings), path_value(node.right, bindings)
        return left / right if isinstance(left, Path) and isinstance(right, str) else None
    if isinstance(node, ast.Attribute):
        base = path_value(node.value, bindings)
        if isinstance(base, dict):
            value = base.get(node.attr)
            if value is AMBIGUOUS:
                raise ValueError('ambiguous imported path constant')
            return value
        if node.attr == 'parent' and isinstance(base, Path):
            return base.parent
    if isinstance(node, ast.Subscript) and isinstance(node.value, ast.Attribute) and node.value.attr == 'parents':
        base, index = path_value(node.value.value, bindings), path_value(node.slice, bindings)
        if isinstance(base, Path) and isinstance(index, int):
            try:
                return base.parents[index]
            except IndexError:
                return None
    if isinstance(node, ast.Call):
        if isinstance(node.func, ast.Name) and node.func.id in {'Path', 'PurePath', 'str'} and len(node.args) == 1:
            value = path_value(node.args[0], bindings)
            # An unanchored Path("src") is not evidence that a fixture reads this repo.
            return value if isinstance(value, Path) else None
        if isinstance(node.func, ast.Attribute):
            base = path_value(node.func.value, bindings)
            if isinstance(base, Path) and node.func.attr in {'resolve', 'absolute'} and not node.args:
                return base.resolve()
            if isinstance(base, Path) and node.func.attr == 'joinpath':
                values = [path_value(arg, bindings) for arg in node.args]
                if all(isinstance(value, str) for value in values):
                    return base.joinpath(*values)
    return None


def python_edges(root: Path, name: str, source: str, paths: set[str], *, module_cache=None) -> tuple[set[str], set[str]] | None:
    try:
        tree = ast.parse(source)
    except SyntaxError:
        return None
    edges: set[str] = set()
    directories: set[str] = set()
    import_roots = {root, (root / name).parent}
    bindings = {'__file__': root / name}
    assignments = [node for node in tree.body if isinstance(node, (ast.Assign, ast.AnnAssign))]
    module_cache = {} if module_cache is None else module_cache
    imported: dict[str, set[Path]] = {}

    def assign(node, values):
        value = path_value(node.value, values) if node.value else None
        targets = node.targets if isinstance(node, ast.Assign) else [node.target]
        for target in targets:
            if isinstance(target, ast.Name):
                if value is None:
                    values.pop(target.id, None)
                else:
                    values[target.id] = value

    def module_values(path):
        if path not in module_cache:
            values = {'__file__': path}
            try:
                nodes = [node for node in ast.parse(path.read_text(encoding='utf-8')).body
                         if isinstance(node, (ast.Assign, ast.AnnAssign))]
            except (OSError, ValueError, SyntaxError):
                nodes = []
            for _ in range(len(nodes) + 1):
                before = dict(values)
                for node in nodes:
                    assign(node, values)
                if values == before:
                    break
            module_cache[path] = values
        return module_cache[path]

    # Resolve module constants before functions that may refer to later constants.
    for _ in range(len(assignments) + 1):
        prior = dict(bindings)
        for node in assignments:
            assign(node, bindings)
        if bindings == prior:
            break

    def record(value, *, directory=False):
        if isinstance(value, Path) and value.is_absolute():
            value = value.resolve()
            if value.is_relative_to(root):
                relative = value.relative_to(root).as_posix()
                if directory:
                    # Directory enumeration also depends on additions/deletions;
                    # no current-file edge can represent that contract.
                    directories.add(relative)
                elif relative in paths and not value.is_dir():
                    edges.add(relative)

    def local_names(function):
        names = {arg.arg for arg in (*function.args.posonlyargs, *function.args.args, *function.args.kwonlyargs)}
        names.update(arg.arg for arg in (function.args.vararg, function.args.kwarg) if arg)
        pending = list(function.body)
        while pending:
            node = pending.pop()
            if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef, ast.ClassDef)):
                names.add(node.name)
                continue
            if isinstance(node, ast.Name) and isinstance(node.ctx, ast.Store):
                names.add(node.id)
            pending.extend(ast.iter_child_nodes(node))
        return names

    def imports(node, values):
        modules = ([alias.name for alias in node.names] if isinstance(node, ast.Import) else
                   [node.module or '', *((node.module + '.' if node.module else '') + alias.name for alias in node.names)])
        roots = import_roots
        if isinstance(node, ast.ImportFrom) and node.level:
            parent = (root / name).parent
            for _ in range(node.level - 1):
                parent = parent.parent
            roots = {parent}
        def remember(module, target):
            if target.is_relative_to(root) and target.relative_to(root).as_posix() in paths and target.is_file():
                imported.setdefault(module, set()).add(target)

        for module in modules:
            if not module:
                continue
            for directory in roots:
                parts = module.split('.')
                for end in range(1, len(parts)):
                    target = directory.joinpath(*parts[:end]) / '__init__.py'
                    record(target)
                    remember('.'.join(parts[:end]), target)
                path = directory.joinpath(*parts)
                record(path.with_suffix('.py'))
                record(path / '__init__.py')
                for target in (path.with_suffix('.py'), path / '__init__.py'):
                    remember(module, target)

        def constants_for(module):
            options = [module_values(path) for path in imported.get(module, ())]
            keys = set().union(*(option.keys() for option in options))
            # Ambiguity matters when a path constant is used, not merely when
            # stateless helpers have the same import name in different roots.
            return {key: options[0].get(key) if all(option.get(key) == options[0].get(key)
                    for option in options) else AMBIGUOUS for key in keys}

        def namespace(module):
            constants = constants_for(module)
            prefix = module + '.'
            children = {name[len(prefix):].split('.')[0] for name in imported if name.startswith(prefix)}
            for child in children:
                constants[child] = namespace(prefix + child)
            return constants

        for alias in node.names:
            binding = alias.asname or alias.name.split('.')[0]
            if isinstance(node, ast.Import):
                if alias.name in imported:
                    values[binding] = namespace(alias.name if alias.asname else binding)
            else:
                constants = constants_for(node.module)
                if alias.name in constants:
                    values[binding] = constants[alias.name]
                else:
                    module = (node.module + '.' if node.module else '') + alias.name
                    if module in imported:
                        values[binding] = namespace(module)

    def visit(node, values):
        if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef)):
            local = {key: value for key, value in values.items() if key not in local_names(node)}
            for statement in node.body:
                visit(statement, local)
            return
        if isinstance(node, (ast.Import, ast.ImportFrom)):
            imports(node, values)
        if isinstance(node, ast.Call) and isinstance(node.func, ast.Attribute) and node.func.attr in {'insert', 'append'}:
            if ast.unparse(node.func.value) == 'sys.path' and node.args:
                directory = path_value(node.args[-1], values)
                if isinstance(directory, Path) and directory.is_absolute() and directory.resolve().is_relative_to(root):
                    import_roots.add(directory.resolve())
        if isinstance(node, ast.Call) and isinstance(node.func, ast.Attribute) and node.func.attr in {'glob', 'rglob', 'iterdir'}:
            record(path_value(node.func.value, values), directory=True)
        if isinstance(node, (ast.BinOp, ast.Call)):
            record(path_value(node, values))
        if isinstance(node, (ast.Assign, ast.AnnAssign)):
            if node.value:
                visit(node.value, values)
            assign(node, values)
            return
        for child in ast.iter_child_nodes(node):
            visit(child, values)

    # Imported constants and later module assignments exist when functions run.
    try:
        for node in tree.body:
            if not isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef)):
                visit(node, bindings)
        for node in tree.body:
            if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef)):
                visit(node, dict(bindings))
    except ValueError:
        return None
    return edges, directories
