FROM ruby:3.3-slim
WORKDIR /app
COPY Gemfile .
RUN bundle install
COPY app.rb .
VOLUME ["/data"]
EXPOSE 3001
CMD ["ruby", "app.rb"]
