require 'sinatra'
require 'sinatra/json'
require 'json'
require 'fileutils'

DATA_DIR = ENV.fetch('DATA_DIR', '/data')

configure do
  set :bind, '0.0.0.0'
  set :port, 3001
  set :show_exceptions, false
end

error do
  status 500
  json error: env['sinatra.error'].message
end

# ── helpers ──────────────────────────────────────────────────────────────────

def ensure_dir(path)
  FileUtils.mkdir_p(path)
end

def memory_path(user_id)
  File.join(DATA_DIR, 'memories', "#{user_id}.md")
end

def notes_path(user_id)
  File.join(DATA_DIR, 'notes', "#{user_id}.json")
end

def skills_path
  File.join(DATA_DIR, 'skills.json')
end

def history_path(user_id)
  File.join(DATA_DIR, 'history', "#{user_id}.jsonl")
end

MAX_TURNS = ENV.fetch('MAX_HISTORY_TURNS', '30').to_i

def trim_history(messages, max_turns)
  cutoff = max_turns * 2
  messages.length > cutoff ? messages.last(cutoff) : messages
end

get '/health' do
  json ok: true
end

# ── memory ───────────────────────────────────────────────────────────────────

get '/memory/:user_id' do
  path = memory_path(params[:user_id])
  json content: File.exist?(path) ? File.read(path) : ''
end

put '/memory/:user_id' do
  body = JSON.parse(request.body.read)
  path = memory_path(params[:user_id])
  ensure_dir(File.dirname(path))
  File.write(path, body.fetch('content', ''))
  json ok: true
end

# ── notes ─────────────────────────────────────────────────────────────────────

def load_notes(user_id)
  path = notes_path(user_id)
  return {} unless File.exist?(path)
  raw = File.read(path).strip
  raw.empty? ? {} : JSON.parse(raw)
rescue JSON::ParserError
  {}
end

def save_notes(user_id, notes)
  path = notes_path(user_id)
  ensure_dir(File.dirname(path))
  File.write(path, JSON.pretty_generate(notes))
end

get '/notes/:user_id' do
  json notes: load_notes(params[:user_id])
end

get '/notes/:user_id/:name' do
  notes = load_notes(params[:user_id])
  name = params[:name]
  halt 404, json(error: 'not found') unless notes.key?(name)
  json content: notes[name]
end

post '/notes/:user_id/:name' do
  body = JSON.parse(request.body.read)
  notes = load_notes(params[:user_id])
  notes[params[:name]] = body.fetch('content', '')
  save_notes(params[:user_id], notes)
  json ok: true
end

delete '/notes/:user_id/:name' do
  notes = load_notes(params[:user_id])
  existed = notes.delete(params[:name])
  save_notes(params[:user_id], notes) if existed
  json ok: true, existed: !existed.nil?
end

# ── skills ────────────────────────────────────────────────────────────────────

def load_skills
  path = skills_path
  return {} unless File.exist?(path)
  raw = File.read(path).strip
  raw.empty? ? {} : JSON.parse(raw)
rescue JSON::ParserError
  {}
end

def save_skills(skills)
  path = skills_path
  ensure_dir(File.dirname(path))
  File.write(path, JSON.pretty_generate(skills))
end

get '/skills' do
  json skills: load_skills
end

get '/skills/:name' do
  skills = load_skills
  name = params[:name]
  halt 404, json(error: 'not found') unless skills.key?(name)
  json skill: skills[name]
end

post '/skills/:name' do
  body = JSON.parse(request.body.read)
  skills = load_skills
  skills[params[:name]] = {
    'name'        => params[:name],
    'description' => body['description'],
    'prompt'      => body.fetch('prompt', ''),
    'created_by'  => body['created_by']
  }.compact
  save_skills(skills)
  json ok: true
end

delete '/skills/:name' do
  skills = load_skills
  existed = skills.delete(params[:name])
  save_skills(skills) if existed
  json ok: true, existed: !existed.nil?
end

# ── history ───────────────────────────────────────────────────────────────────

def load_history(user_id)
  path = history_path(user_id)
  return [] unless File.exist?(path)
  messages = File.readlines(path, chomp: true)
              .map(&:strip)
              .reject(&:empty?)
              .filter_map { |l| JSON.parse(l) rescue nil }
  trim_history(messages, MAX_TURNS)
end

def save_history(user_id, messages)
  path = history_path(user_id)
  ensure_dir(File.dirname(path))
  File.write(path, messages.map { |m| JSON.generate(m) }.join("\n") + "\n")
end

get '/history/:user_id' do
  json messages: load_history(params[:user_id])
end

post '/history/:user_id' do
  body = JSON.parse(request.body.read)
  user_message      = body.fetch('user_message')
  assistant_messages = body.fetch('assistant_messages', [])
  history = load_history(params[:user_id])
  history << user_message
  history.concat(assistant_messages)
  history = trim_history(history, MAX_TURNS)
  save_history(params[:user_id], history)
  json messages: history
end

delete '/history/:user_id' do
  path = history_path(params[:user_id])
  File.delete(path) if File.exist?(path)
  json ok: true
end
