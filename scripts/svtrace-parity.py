#!/usr/bin/env python3
"""Asks svtrace and the patched slang-server (aviyn/slang-server, branch cone-connection-sites) the drivers,
loads and drivers-with-constants of every signal of a design, and counts the answers that differ.

    scripts/svtrace-parity.py WORKSPACE [SVTRACE [SERVER]] [--slang PATH] [--limit N] [--show N] [--dump FILE]
    scripts/svtrace-parity.py WORKSPACE [SVTRACE [SERVER]] --hierarchy [--limit N] [--show N]

WORKSPACE is a folder with `.slang/server.json`. Defaults: svtrace at $CARGO_TARGET_DIR/release/svtrace (else the
repository's target/), the server at ~/tools/slang-server-patched/slang-server, slang at ~/tools/slang/slang. Python 3,
standard library only.

The fork has no `clock` site kind, so before the answers are compared svtrace's are put in the fork's terms:
in drivers and constants the clock sites (and a driver with no site left) are dropped; in loads a clock site is
a `data` site, where the fork reports a signal read in an event control. Answers that differ only for that
reason are counted on their own ("clock kind only"), as are the clock sites svtrace answers in all. What still
differs is explained, where the design's AST dump can be made, as one of the known causes:
  - clock leak: a fork build before 8fb7256 keeps a read in an event control across the blocks of a module;
  - uninstantiated: the fork walks generate branches slang does not elaborate;
  - one site per statement: the fork lists a signal's every read in an assignment, port connection or condition,
    svtrace one of each kind per statement (the first by position), so the several lines of a statement are one row;
and anything else is listed as unexplained.

With --hierarchy the two are asked for the design hierarchy instead (slang.getScope and slang.getScopes): the roots,
then every scope found in either one's answers (instances, instance arrays, generate blocks and arrays, interface
ports, packages), and slang.getScopes for each of those paths. The answers are compared item by item and the
differences counted by field and kind of item; locations are compared by file and start only (svtrace has the
position of a symbol's name, the fork the range of its syntax), and the differences in where a range ends are
counted on their own.
"""
import argparse
import json
import os
import shlex
import subprocess
import sys
import tempfile
import time

HOME = os.path.expanduser('~')
# Cargo's build folder: CARGO_TARGET_DIR when set, else the repository's `target`.
TARGET = os.environ.get('CARGO_TARGET_DIR') or os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), 'target')
COMMANDS = ['slang.getDriversWithLocation', 'slang.getLoadsWithLocation', 'slang.getDriversWithConstants']


class Client:
    """A tiny LSP client over a child process's stdio."""

    def __init__(self, cmd, root, log_dir, name):
        self.root = root
        self.id = 0
        self.log = open(os.path.join(log_dir, name + '.err'), 'wb')
        self.p = subprocess.Popen(cmd, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=self.log)

    def send(self, msg):
        body = json.dumps(msg).encode()
        self.p.stdin.write(b'Content-Length: %d\r\n\r\n' % len(body) + body)
        self.p.stdin.flush()

    def read(self):
        length = None
        while True:
            line = self.p.stdout.readline()
            if not line:
                return None
            line = line.strip()
            if not line:
                break
            if line.lower().startswith(b'content-length'):
                length = int(line.split(b':')[1])
        return json.loads(self.p.stdout.read(length))

    def request(self, method, params):
        self.id += 1
        mine = self.id
        self.send({'jsonrpc': '2.0', 'id': mine, 'method': method, 'params': params})
        while True:
            msg = self.read()
            if msg is None:
                raise RuntimeError('the server ended (see %s)' % self.log.name)
            if msg.get('id') == mine and 'method' not in msg:
                return msg
            if 'method' in msg and 'id' in msg:  # a request from the server
                self.send({'jsonrpc': '2.0', 'id': msg['id'], 'result': None})

    def notify(self, method, params):
        self.send({'jsonrpc': '2.0', 'method': method, 'params': params})

    def start(self):
        uri = 'file://' + self.root
        self.request('initialize', {
            'processId': os.getpid(), 'rootUri': uri,
            'capabilities': {'workspace': {'workspaceFolders': True}},
            'workspaceFolders': [{'uri': uri, 'name': 'w'}]})
        self.notify('initialized', {})

    def command(self, command, *arguments):
        return self.request('workspace/executeCommand', {'command': command, 'arguments': list(arguments)})

    def stop(self):
        try:
            self.request('shutdown', None)
            self.notify('exit', None)
        except Exception:
            pass
        self.p.kill()


# ---- Answers, put in comparable form ----

def place(location):
    start, end = location['range']['start'], location['range']['end']
    return (os.path.basename(location['uri']), start['line'], start['character'], end['line'], end['character'])


def sites_of(entry, mode):
    """The sites of an entry as (place, kind); `mode` says what to do with clock sites:
    'keep', 'drop' (drivers) or 'data' (loads: the fork's name for a read in an event control)."""
    out = []
    for site in entry['sites']:
        kind = site['kind']
        if kind == 'clock':
            if mode == 'drop':
                continue
            if mode == 'data':
                kind = 'data'
        out.append((place(site['location']), kind))
    return tuple(out)


def normalize(result, mode):
    """A list answer as [(path, declaration, sites)], or a {drivers, constants} answer as a pair."""
    if isinstance(result, dict):
        constants = [(place(c['location']), c['kind']) for c in result['constants']]
        return {'drivers': normalize(result['drivers'], mode), 'constants': constants}
    entries = []
    for entry in result:
        sites = sites_of(entry, mode)
        if mode == 'drop' and not sites:
            continue
        entries.append((entry['path'], place(entry['location']), sites))
    return entries


def leaves(answer):
    return {entry[0]: entry for entry in (answer['drivers'] if isinstance(answer, dict) else answer)}


def clock_count(result):
    entries = result['drivers'] if isinstance(result, dict) else result
    return sum(1 for entry in entries for site in entry['sites'] if site['kind'] == 'clock')


def equal(a, b):
    if isinstance(a, dict):
        return sorted(a['drivers']) == sorted(b['drivers']) and a['constants'] == b['constants']
    return sorted(a) == sorted(b)


# ---- The design's AST, to explain differences ----

def read_config(workspace):
    path = os.path.join(workspace, '.slang', 'server.json')
    try:
        with open(path) as f:
            return json.load(f)
    except (OSError, ValueError):
        return {}


def make_dump(workspace, slang):
    """slang's AST dump of the workspace's design (the workspace layer of server.json only)."""
    config = read_config(workspace)
    if not os.access(slang, os.X_OK) or not config.get('build'):
        return None
    flags = config.get('flags', '')
    flags = shlex.split(flags) if isinstance(flags, str) else list(flags)
    cmd = [slang] + flags + ['-f', config['build'], '--ast-json', '-', '--ast-json-source-info',
                             '--disable-instance-caching', '-q']
    run = subprocess.run(cmd, cwd=workspace, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, env={**os.environ, **config.get('env', {})})
    try:
        return json.loads(run.stdout)
    except ValueError:
        return None


class Reads:
    """Where the dump reads a signal inside a continuous assign, a procedural block or an instance's
    connections, by the chain of instance names it is in (a module can be instantiated with different
    parameters, so a place can be elaborated in one instance and not in another): `values`, and
    `timing` for the reads in an event control, and `statements` the statement that holds each read: the
    innermost assignment, port connection, condition (an `if`'s, a `case`'s selector, a case item's expressions)
    or event control, by the place it starts (None when it is none of those). `chains` are the instance chains
    there are."""

    def __init__(self, dump):
        self.values, self.timing, self.chains, self.statements = set(), set(), set(), {}
        self.walk(dump['design'], ())

    @staticmethod
    def start(node, role):
        return (role, os.path.basename(node['source_file_start']), node['source_line_start'], node['source_column_start'])

    def inner(self, node, kind, key, value, statement):
        """The statement that holds what is under `node[key]`."""
        placed = lambda n: isinstance(n, dict) and 'source_line_start' in n
        if kind == 'Conditional' and key == 'conditions' and placed(node):
            return self.start(node, 'if')
        if kind == 'Case' and key == 'expr' and placed(node):
            return self.start(node, 'case')
        if key == 'expressions' and 'stmt' in node and isinstance(value, list) and value and placed(value[0]):
            return self.start(value[0], 'item')
        if kind == 'Timed' and key == 'timing' and placed(value):
            return self.start(value, 'timing')
        return statement

    def walk(self, node, chain, in_timing=False, in_container=False, statement=None):
        if isinstance(node, dict):
            kind = node.get('kind')
            if kind == 'Instance':
                # The connections are written in the parent's scope; the body is the instance's.
                inner = chain + (node['name'],) if node.get('name') else chain
                self.chains.add(inner)
                for key, value in node.items():
                    if key == 'connections':
                        for connection in value:
                            if 'expr' in connection:
                                expr = connection['expr']
                                self.walk(expr, chain, False, True, self.start(expr, 'connection') if 'source_line_start' in expr else None)
                    else:
                        self.walk(value, inner, False, False, None)
                return
            if kind in ('ContinuousAssign', 'ProceduralBlock'):
                in_container = True
            if kind == 'Assignment' and 'source_line_start' in node:
                statement = self.start(node, 'assign')
            if kind in ('NamedValue', 'HierarchicalValue') and 'source_line_start' in node and in_container:
                key = (chain, os.path.basename(node['source_file_start']), node['source_line_start'] - 1,
                       node['source_column_start'] - 1)
                self.values.add(key)
                self.statements[(chain,) + key[1:]] = statement
                if in_timing:
                    self.timing.add(key)
            if kind in ('SignalEvent', 'EventList'):
                in_timing = True
            for key, value in node.items():
                self.walk(value, chain, in_timing, in_container, self.inner(node, kind, key, value, statement))
        elif isinstance(node, list):
            for value in node:
                self.walk(value, chain, in_timing, in_container, statement)

    def chain_of(self, path):
        """The deepest instance chain a hierarchical path goes through (generate blocks and array
        indexes are passed over)."""
        chain = ()
        for part in path.split('.')[:-1]:
            name = part.split('[')[0]
            if chain + (name,) in self.chains or not chain:
                chain += (name,)
        return chain

    def read(self, table, paths, key):
        return any((self.chain_of(path),) + key in table for path in paths)

    def statement(self, paths, key):
        """The statement that holds the read at `key` (file, line, column), or None."""
        for path in paths:
            found = self.statements.get((self.chain_of(path),) + key)
            if found:
                return found
        return None


def same_statement(reads, paths, key, kind, mine):
    """Whether svtrace has a site of this kind in the statement that holds the read at `key`, which is why it
    has not the fork's site of it: one site of a kind per statement."""
    statement = reads.statement(paths, key)
    return statement is not None and mine is not None and any(
        k == kind and reads.statement(paths, place[:3]) == statement for place, k in mine[2])


def explain(reads, asked, oracle, svtrace):
    """The causes of a difference between two normalized answers for the signal `asked`: (cause, detail) pairs."""
    causes = []
    la, lb = leaves(oracle), leaves(svtrace)
    for path in sorted(set(lb) - set(la)):
        causes.append(('unexplained', 'only svtrace: ' + path))
    for path, (_, _, sites_a) in la.items():
        sites_b = set(lb[path][2]) if path in lb else set()
        for site in sites_a:
            if site in sites_b:
                continue
            key = site[0][:3]
            if reads is None:
                causes.append(('unexplained', 'only the fork: %s %s' % (path, site)))
            elif not reads.read(reads.values, (asked, path), key):
                causes.append(('uninstantiated', path))
            elif same_statement(reads, (asked, path), key, site[1], lb.get(path)):
                causes.append(('one site per statement', path))
            elif reads.read(reads.timing, (asked, path), key):
                causes.append(('clock leak', path))
            else:
                causes.append(('unexplained', 'only the fork: %s %s' % (path, site)))
        if path in lb:
            for site in sorted(sites_b - set(sites_a)):
                causes.append(('unexplained', 'only svtrace: %s %s' % (path, site)))
    if isinstance(oracle, dict) and oracle['constants'] != svtrace['constants']:
        causes.append(('unexplained', 'constants differ'))
    return causes


# ---- The comparison ----

def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument('workspace')
    parser.add_argument('svtrace', nargs='?', default=os.path.join(TARGET, 'release/svtrace'))
    parser.add_argument('server', nargs='?', default=os.path.join(HOME, 'tools/slang-server-patched/slang-server'))
    parser.add_argument('--slang', default=os.path.join(HOME, 'tools/slang/slang'))
    parser.add_argument('--dump', help='slang --ast-json output of the design, to explain differences (default: made)')
    parser.add_argument('--limit', type=int, default=None, help='only the first N signals')
    parser.add_argument('--show', type=int, default=5, help='examples to print per kind of difference')
    parser.add_argument('--hierarchy', action='store_true', help='compare the hierarchy (getScope, getScopes) instead of the traces')
    args = parser.parse_args()
    workspace = os.path.abspath(os.path.expanduser(args.workspace))
    for what, path in (('svtrace', args.svtrace), ('the patched server', args.server)):
        if not os.access(path, os.X_OK):
            sys.exit('no %s at %s' % (what, path))

    logs = tempfile.mkdtemp(prefix='svtrace-parity-')
    oracle = Client([args.server], workspace, logs, 'slang-server')
    mine = Client([args.svtrace], workspace, logs, 'svtrace')
    try:
        oracle.start()
        mine.start()
        if args.hierarchy:
            # The server answers once its own build is done.
            for _ in range(300):
                r = oracle.command('slang.getScopes', '')
                if r.get('result') or 'error' in r:
                    break
                time.sleep(1)
            compare_hierarchy(oracle, mine, args)
            return
        signals = mine.command('svtrace.listSignals')['result']
        if args.limit:
            signals = signals[:args.limit]
        print('%d signals in %s' % (len(signals), workspace))
        # The server answers once its own build is done.
        for _ in range(300):
            r = oracle.command(COMMANDS[0], signals[0])
            if r.get('result') is not None or 'error' in r:
                break
            time.sleep(1)
        compare(oracle, mine, signals, args, workspace)
    finally:
        oracle.stop()
        mine.stop()
    print('server logs: %s' % logs)


# ---- The hierarchy ----

SCOPE_KINDS = ('Instance', 'InstanceArray', 'Package', 'Scope', 'ScopeArray', 'InterfacePort', 'InterfacePortArray')


def join_path(parent, name, parent_kind):
    """The path of a child as the hierarchy view (nvim-bridge slang/hierarchy.lua) joins it."""
    if parent == '':
        return name
    if name.startswith('['):
        return parent + name
    return parent + ('::' if parent_kind == 'Package' else '.') + name


def where(location):
    if not location:
        return None
    start, end = location['range']['start'], location['range']['end']
    return (os.path.basename(location['uri']), start['line'], start['character']), (end['line'], end['character'])


class HierarchyStats:
    def __init__(self):
        self.items = 0
        self.same = 0
        self.fields = {}      # (field, kind) -> count
        self.examples = {}    # (field, kind) -> [text]
        self.lists = {}       # how a children list differs -> [text]
        self.ends = {}        # kind -> count of items whose range ends elsewhere (only counted when the start agrees)

    def field(self, name, kind, text):
        self.fields[(name, kind)] = self.fields.get((name, kind), 0) + 1
        self.examples.setdefault((name, kind), []).append(text)

    def list_difference(self, what, text):
        self.lists.setdefault(what, []).append(text)


def compare_items(path, a, b, stats):
    """Compares two lists of items, and for items in both their fields and, recursively, their children."""
    keys_a = [(i['kind'], i['instName']) for i in a]
    keys_b = [(i['kind'], i['instName']) for i in b]
    if keys_a != keys_b:
        only_a = [k for k in keys_a if k not in keys_b]
        only_b = [k for k in keys_b if k not in keys_a]
        if only_a:
            stats.list_difference('only the fork', '%s: %s' % (path, only_a[:6]))
        if only_b:
            stats.list_difference('only svtrace', '%s: %s' % (path, only_b[:6]))
        if not only_a and not only_b:
            stats.list_difference('same items, other order', '%s: fork %s, svtrace %s' % (path, [k[1] for k in keys_a[:8]], [k[1] for k in keys_b[:8]]))
    by_b = {}
    for item in b:
        by_b.setdefault((item['kind'], item['instName']), []).append(item)
    used = {}
    for item in a:
        key = (item['kind'], item['instName'])
        n = used.get(key, 0)
        used[key] = n + 1
        if n >= len(by_b.get(key, [])):
            continue
        other = by_b[key][n]
        stats.items += 1
        before = sum(stats.fields.values())
        here = '%s/%s' % (path, item['instName'])
        for name in ('type', 'value', 'declName', 'declKind', 'hasChildren'):
            if item.get(name) != other.get(name):
                stats.field(name, item['kind'], '%s: fork %r, svtrace %r' % (here, item.get(name), other.get(name)))
        for name in ('instLoc', 'declLoc'):
            wa, wb = where(item.get(name)), where(other.get(name))
            if wa is None and wb is None:
                continue
            if wa is None or wb is None:
                stats.field(name + ' missing', item['kind'], here)
                continue
            if wa[0][0] != wb[0][0]:
                stats.field(name + ' file', item['kind'], '%s: fork %s, svtrace %s' % (here, wa[0][0], wb[0][0]))
            elif wa[0] != wb[0]:
                stats.field(name + ' start', item['kind'], '%s: fork %s, svtrace %s' % (here, wa[0][1:], wb[0][1:]))
            elif wa[1] != wb[1]:
                stats.ends[item['kind']] = stats.ends.get(item['kind'], 0) + 1
        if sum(stats.fields.values()) == before:
            stats.same += 1
        if 'children' in item or 'children' in other:
            compare_items(here, item.get('children', []), other.get('children', []), stats)


def scope_paths(items, path, parent_kind, out):
    """The paths of the items that can be asked for their children (and of those inside them), with the first step's."""
    for item in items:
        here = join_path(path, item['instName'], parent_kind)
        if item['kind'] in SCOPE_KINDS:
            out.add(here)
        scope_paths(item.get('children', []), here, item['kind'], out)


def compare_hierarchy(oracle, mine, args):
    stats = HierarchyStats()
    errors = []
    answers = {}

    def ask(client, command, path):
        r = client.command(command, path)
        if 'error' in r:
            return None, r['error'].get('message')
        return r.get('result'), None

    paths = set()
    packages = set()
    for who, client in (('fork', oracle), ('svtrace', mine)):
        steps, error = ask(client, 'slang.getScopes', '')
        if error or not steps:
            errors.append('%s getScopes(""): %s' % (who, error or 'no answer'))
            continue
        answers[who] = steps
        packages.update(i['instName'] for i in steps[0]['children'] if i['kind'] == 'Package')
        scope_paths(steps[0]['children'], '', '', paths)
    if len(answers) == 2:
        compare_items('<roots>', answers['fork'][0]['children'], answers['svtrace'][0]['children'], stats)

    # Every scope in either answer, and what each one's children show.
    pending = sorted(paths)
    done = set()
    scopes_compared = steps_compared = step_path_differences = 0
    step_examples = []
    while pending:
        path = pending.pop(0)
        if path in done or (args.limit and len(done) >= args.limit):
            continue
        done.add(path)
        a, ea = ask(oracle, 'slang.getScope', path)
        b, eb = ask(mine, 'slang.getScope', path)
        if ea or eb:
            errors.append('getScope(%r): fork %r, svtrace %r' % (path, ea, eb))
            continue
        scopes_compared += 1
        compare_items(path, a or [], b or [], stats)
        found = set()
        kind = 'Package' if path in packages else ''
        scope_paths(a or [], path, kind, found)
        scope_paths(b or [], path, kind, found)
        pending.extend(sorted(found - done))
        # The steps down to the path.
        sa, ea = ask(oracle, 'slang.getScopes', path)
        sb, eb = ask(mine, 'slang.getScopes', path)
        if ea or eb:
            errors.append('getScopes(%r): fork %r, svtrace %r' % (path, ea, eb))
            continue
        steps_compared += 1
        pa, pb = [s['path'] for s in sa], [s['path'] for s in sb]
        if pa != pb:
            step_path_differences += 1
            step_examples.append('%s: fork %s, svtrace %s' % (path, pa, pb))
        for x, y in zip(sa[1:], sb[1:]):
            if x['path'] == y['path']:
                compare_items('steps ' + x['path'], x['children'], y['children'], stats)

    print('%d scopes asked with getScope and getScopes (%d steps lists compared)' % (scopes_compared, steps_compared))
    print('%d items in both answers: %d with every field equal' % (stats.items, stats.same))
    print('getScopes step paths that differ: %d' % step_path_differences)
    for text in step_examples[:args.show]:
        print('    ' + text[:400])
    if stats.lists:
        print('children lists that differ:')
        for what, texts in sorted(stats.lists.items()):
            print('  %s (%d):' % (what, len(texts)))
            for text in texts[:args.show]:
                print('    ' + text[:400])
    if stats.fields:
        print('fields that differ (field, kind of item: count):')
        for (name, kind), count in sorted(stats.fields.items()):
            print('  %s, %s: %d' % (name, kind, count))
            for text in stats.examples[(name, kind)][:args.show]:
                print('      ' + text[:400])
    if stats.ends:
        print('ranges that end elsewhere, the start being the same (by kind): ' + ', '.join('%s %d' % kv for kv in sorted(stats.ends.items())))
    if errors:
        print('errors (%d):' % len(errors))
        for text in errors[:args.show]:
            print('  ' + text[:400])


def compare(oracle, mine, signals, args, workspace):
    dump = None
    if args.dump:
        with open(args.dump) as f:
            dump = json.load(f)
    else:
        dump = make_dump(workspace, args.slang)
    reads = Reads(dump) if dump else None
    if reads is None:
        print('no AST dump: differences are not classified')

    stats = {c: {'compared': 0, 'same': 0, 'order only': 0, 'clock kind only': 0, 'differ': 0, 'errors differ': 0, 'empty vs no-reference error': 0,
                 'clock sites': 0, 'answers with clock sites': 0} for c in COMMANDS}
    causes = {c: {} for c in COMMANDS}
    examples = {c: {} for c in COMMANDS}
    order_only = {c: [] for c in COMMANDS}

    def example(command, kind, text):
        examples[command].setdefault(kind, []).append(text)

    for path in signals:
        for command in COMMANDS:
            st = stats[command]
            a, b = oracle.command(command, path), mine.command(command, path)
            st['compared'] += 1
            no_reference = False
            if 'error' in a or 'error' in b:
                ea, eb = a.get('error', {}).get('message'), b.get('error', {}).get('message')
                if ea == eb:
                    st['same'] += 1
                    continue
                if ea is None and (eb or '').startswith('Could not find reference to'):
                    # svtrace says nothing reads the signal: an empty answer, to be compared (the fork's is
                    # empty, or lists reads in a generate branch that is not elaborated).
                    no_reference = True
                else:
                    st['errors differ'] += 1
                    example(command, 'errors differ', '%s: fork %r, svtrace %r' % (path, ea, eb))
                    continue
            ra, rb = (a.get('result') or [], []) if no_reference else (a['result'], b['result'])
            clocks = clock_count(rb)
            st['clock sites'] += clocks
            st['answers with clock sites'] += 1 if clocks else 0
            mode = 'data' if command == COMMANDS[1] else 'drop'
            na, nb = normalize(ra, 'keep'), normalize(rb, mode)
            raw = normalize(rb, 'keep')
            if equal(na, nb):
                if no_reference:
                    st['empty vs no-reference error'] += 1
                elif not equal(na, raw):
                    st['clock kind only'] += 1
                elif na != nb:
                    st['order only'] += 1
                    order_only[command].append(path)
                else:
                    st['same'] += 1
                continue
            st['differ'] += 1
            found = explain(reads, path, na, nb)
            kinds = sorted({k for k, _ in found}) or ['unexplained']
            for kind in kinds:
                causes[command][kind] = causes[command].get(kind, 0) + 1
            for kind, detail in found:
                if kind == 'unexplained':
                    example(command, 'unexplained', '%s: %s' % (path, detail))
            if not found:
                example(command, 'unexplained', '%s: differs, no cause found' % path)

    print()
    for command in COMMANDS:
        st = stats[command]
        print(command)
        print('  compared %(compared)d: same %(same)d, order only %(order only)d, clock kind only %(clock kind only)d, '
              'differ %(differ)d, errors differ %(errors differ)d, empty vs no-reference error %(empty vs no-reference error)d' % st)
        print('  svtrace answered %(clock sites)d clock sites in %(answers with clock sites)d answers (the fork has no clock kind)' % st)
        if causes[command]:
            print('  differing answers by cause (one answer can have several): ' +
                  ', '.join('%s %d' % kv for kv in sorted(causes[command].items())))
        for kind, texts in examples[command].items():
            print('  %s (%d):' % (kind, len(texts)))
            for text in texts[:args.show]:
                print('    ' + text[:600])
        if order_only[command]:
            print('  order only, e.g. ' + ', '.join(order_only[command][:args.show]))
    unexplained = sum(len(examples[c].get('unexplained', [])) + len(examples[c].get('errors differ', [])) for c in COMMANDS)
    print('\n%d unexplained or error differences' % unexplained)


if __name__ == '__main__':
    main()
