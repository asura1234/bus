"""Resolve Python source imports and paths without executing target modules."""
from __future__ import annotations

import ast
from pathlib import Path


def path_value(node, bindings):
    if isinstance(node, ast.Constant) and isinstance(node.value, (str, int)):
        return node.value
    if isinstance(node, ast.Name):
        return bindings.get(node.id)
    if isinstance(node, ast.BinOp) and isinstance(node.op, ast.Div):
        left, right = path_value(node.left, bindings), path_value(node.right, bindings)
        return left / right if isinstance(left, Path) and isinstance(right, str) else None
    if isinstance(node, ast.Attribute) and node.attr == 'parent':
        base = path_value(node.value, bindings)
        return base.parent if isinstance(base, Path) else None
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


def python_edges(root: Path, name: str, source: str, paths: set[str]) -> set[str] | None:
    try:
        tree = ast.parse(source)
    except SyntaxError:
        return None
    edges: set[str] = set()
    import_roots = {root, (root / name).parent}
    bindings = {'__file__': root / name}
    assignments = [node for node in tree.body if isinstance(node, (ast.Assign, ast.AnnAssign))]

    def assign(node, values):
        value = path_value(node.value, values) if node.value else None
        targets = node.targets if isinstance(node, ast.Assign) else [node.target]
        for target in targets:
            if isinstance(target, ast.Name):
                if value is None:
                    values.pop(target.id, None)
                else:
                    values[target.id] = value

    # Resolve module constants before functions that may refer to later constants.
    for _ in range(len(assignments) + 1):
        prior = dict(bindings)
        for node in assignments:
            assign(node, bindings)
        if bindings == prior:
            break

    def record(value):
        if isinstance(value, Path) and value.is_absolute():
            value = value.resolve()
            if value.is_relative_to(root):
                relative = value.relative_to(root).as_posix()
                if relative in paths and not value.is_dir():
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

    def imports(node):
        modules = ([alias.name for alias in node.names] if isinstance(node, ast.Import) else
                   [node.module or '', *((node.module + '.' if node.module else '') + alias.name for alias in node.names)])
        roots = import_roots
        if isinstance(node, ast.ImportFrom) and node.level:
            parent = (root / name).parent
            for _ in range(node.level - 1):
                parent = parent.parent
            roots = {parent}
        for module in modules:
            if not module:
                continue
            for directory in roots:
                parts = module.split('.')
                for end in range(1, len(parts)):
                    record(directory.joinpath(*parts[:end]) / '__init__.py')
                path = directory.joinpath(*parts)
                record(path.with_suffix('.py'))
                record(path / '__init__.py')

    def visit(node, values):
        if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef)):
            local = {key: value for key, value in values.items() if key not in local_names(node)}
            for statement in node.body:
                visit(statement, local)
            return
        if isinstance(node, (ast.Import, ast.ImportFrom)):
            imports(node)
        if isinstance(node, ast.Call) and isinstance(node.func, ast.Attribute) and node.func.attr in {'insert', 'append'}:
            if ast.unparse(node.func.value) == 'sys.path' and node.args:
                directory = path_value(node.args[-1], values)
                if isinstance(directory, Path) and directory.is_absolute() and directory.resolve().is_relative_to(root):
                    import_roots.add(directory.resolve())
        if isinstance(node, (ast.BinOp, ast.Call)):
            record(path_value(node, values))
        if isinstance(node, (ast.Assign, ast.AnnAssign)):
            if node.value:
                visit(node.value, values)
            assign(node, values)
            return
        for child in ast.iter_child_nodes(node):
            visit(child, values)

    for node in tree.body:
        visit(node, dict(bindings) if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef)) else bindings)
    return edges
